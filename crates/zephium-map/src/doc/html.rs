use super::*;
use arena::NodeRef;
use std::borrow::Cow;
mod arena;
mod decode;
mod sink;

const GENERIC_TEXT_CHILDREN: usize = 8;

impl Doc {
    pub fn from_html(
        page: &Url,
        bytes: &[u8],
        charset_label: Option<&str>,
        limits: DocLimits,
    ) -> Result<Self, DocError> {
        let mut builder = DocBuilder::new(page, limits)?;
        if bytes.len() > limits.input_bytes {
            return Err(DocError::Input);
        }
        let dom = decode::parse(bytes, charset_label, limits);
        let mut base = page.clone();
        let Some(root) = dom
            .node(0)
            .children()
            .find(|n| tag_is(n, "html") && !hidden(n))
        else {
            builder.doc.truncated |= dom.stopped || dom.lifted;
            return Ok(builder.finish());
        };
        if let Some(head) = root.children().find(|n| tag_is(n, "head") && !hidden(n)) {
            if let Some(element) = head
                .children()
                .find(|n| tag_is(n, "base") && !hidden(n) && n.has_attr("href"))
            {
                if let Some(url) = builder
                    .resolve(&element, "href", page)
                    .filter(|u| u.origin() == page.origin())
                {
                    base = url;
                }
            }
            if builder.doc.truncated {
                return Err(DocError::Url(KeyError::UrlLength));
            }
            for n in head.children().filter(|n| !n.is_text() && !hidden(n)) {
                builder.head_element(&n, &base);
            }
            builder.doc.head.truncated = std::mem::take(&mut builder.doc.truncated);
        }
        let body = root.children().find(|n| tag_is(n, "body")).or_else(|| {
            (dom.lifted && limits.arena_depth == 1)
                .then(|| dom.node(0).children().find(|n| tag_is(n, "body")))
                .flatten()
        });
        if let Some(body) = body {
            builder.walk(body, &base)?;
        }
        builder.doc.truncated |= dom.stopped || dom.lifted;
        Ok(builder.finish())
    }
}

fn tag_is(node: &NodeRef, tag: &str) -> bool {
    node.tag() == tag
}
fn hidden(n: &NodeRef) -> bool {
    n.hidden_by_lift()
        || n.has_attr("hidden")
        || n.attr("aria-hidden")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"))
        || tag_is(n, "input")
            && n.attr("type")
                .is_some_and(|v| v.eq_ignore_ascii_case("hidden"))
}

impl DocBuilder {
    fn resolve(&mut self, node: &NodeRef, attr: &str, base: &Url) -> Option<Url> {
        let raw = node.attr(attr)?;
        if node.clipped(attr) || raw.len() > self.limits.url_bytes {
            self.mark_truncated();
            return None;
        }
        if raw.trim().is_empty() {
            return None;
        }
        let url = base.join(raw).ok()?;
        match check_url(&url, self.limits.url_bytes) {
            Ok(()) => Some(url),
            Err(KeyError::UrlLength) => {
                self.mark_truncated();
                None
            }
            Err(_) => None,
        }
    }

    fn bounded_prefix<'a>(&mut self, value: &'a str, room: usize) -> &'a str {
        let end = value.floor_char_boundary(room.min(self.limits.text_bytes - self.text_bytes));
        if end < value.len() {
            self.mark_truncated();
        }
        &value[..end]
    }

    fn head_text(&mut self, value: &str, room: usize) -> String {
        let text = self.bounded_prefix(value, room);
        self.text_bytes += text.len();
        text.to_owned()
    }

    fn head_element(&mut self, node: &NodeRef, base: &Url) {
        let limit = self.limits.node_text_bytes;
        if node.loss() {
            self.mark_truncated();
        }
        match node.tag() {
            "title" if self.doc.head.title.is_empty() => {
                let raw = node
                    .children()
                    .find(|c| c.is_text())
                    .map_or("", |c| c.text());
                let value = self.words(raw);
                self.doc.head.title = self.head_text(&value, limit);
            }
            "meta"
                if self.doc.head.site_name.is_empty()
                    && node
                        .attr("property")
                        .is_some_and(|v| v.eq_ignore_ascii_case("og:site_name")) =>
            {
                if let Some(value) = node.attr("content") {
                    let text = self.words(value);
                    self.doc.head.site_name = self.head_text(&text, limit);
                }
            }
            "link" => {
                let rel = node.attr("rel").unwrap_or_default();
                let has = |word: &str| {
                    rel.split_ascii_whitespace()
                        .any(|v| v.eq_ignore_ascii_case(word))
                };
                if has("canonical") && self.doc.head.canonical.is_none() {
                    self.doc.head.canonical = self.resolve(node, "href", base);
                }
                if has("alternate") || has("feed") {
                    if self.doc.head.alternates.len() >= MAX_HEAD_ITEMS {
                        self.mark_truncated();
                        return;
                    }
                    if let Some(url) = self.resolve(node, "href", base) {
                        let mut attr =
                            |name| self.head_text(node.attr(name).unwrap_or_default(), limit);
                        let alternate = Alternate {
                            url,
                            media_type: attr("type"),
                            language: attr("hreflang"),
                            title: attr("title"),
                        };
                        self.doc.head.alternates.push(alternate);
                    }
                }
            }
            "script"
                if node
                    .attr("type")
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/ld+json")) =>
            {
                let raw = node
                    .children()
                    .find(|c| c.is_text())
                    .map_or("", |c| c.text());
                if node.loss()
                    || self.doc.head.linked_data.len() >= MAX_HEAD_ITEMS
                    || raw.len() > limit.min(self.limits.text_bytes - self.text_bytes)
                {
                    self.mark_truncated();
                } else {
                    self.text_bytes += raw.len();
                    self.doc.head.linked_data.push(raw.to_string());
                }
            }
            _ => (),
        }
    }

    fn append(&mut self, at: usize, raw: &str, verbatim: bool) {
        let node = &self.doc.nodes[at];
        let room = self.limits.node_text_bytes
            - node.text.len()
            - node.name.len()
            - node.element_id.as_deref().map_or(0, str::len);
        let separator = !verbatim && !node.text.is_empty();
        let room = room.min(self.limits.text_bytes - self.text_bytes);
        let extra = usize::from(separator);
        let end = raw.floor_char_boundary(room.saturating_sub(extra));
        if end < raw.len() {
            self.mark_truncated();
        }
        if end > 0 {
            self.text_bytes += extra + end;
            let dest = &mut self.doc.nodes[at].text;
            if separator {
                dest.push(' ');
            }
            dest.push_str(&raw[..end]);
        }
    }

    fn words(&mut self, raw: &str) -> String {
        let mut out = String::new();
        let cap = self
            .limits
            .node_text_bytes
            .min(self.limits.text_bytes - self.text_bytes);
        for word in raw
            .split(|c: char| c.is_control() || c.is_whitespace())
            .filter(|word| !word.is_empty())
        {
            let gap = usize::from(!out.is_empty());
            let end = word.floor_char_boundary(cap.saturating_sub(out.len() + gap));
            if end != 0 {
                if gap != 0 {
                    out.push(' ')
                }
                out.push_str(&word[..end]);
            }
            if end < word.len() {
                self.mark_truncated();
                break;
            }
        }
        out
    }

    fn capture_text(
        &mut self,
        parent: Option<usize>,
        raw: &str,
        verbatim: bool,
        level: usize,
    ) -> Result<(), DocError> {
        let mut ancestor = parent;
        let words = if verbatim {
            Cow::Borrowed(raw)
        } else {
            Cow::Owned(self.words(raw))
        };
        if words.is_empty() {
            return Ok(());
        }
        let mut captured = false;
        let mut inline = false;
        let mut prose = false;
        while let Some(at) = ancestor {
            let n = &self.doc.nodes[at];
            ancestor = n.parent;
            let takes_prose = matches!(
                n.role,
                Role::Paragraph
                    | Role::ListItem
                    | Role::Document
                    | Role::Cell
                    | Role::CellHeader
                    | Role::CodeBlock
            );
            if n.role == Role::Text && !inline
                || !prose && takes_prose
                || matches!(n.role, Role::Heading(_) | Role::Link | Role::Button)
            {
                captured = true;
                self.append(at, &words, verbatim);
            }
            inline |= self.doc.nodes[at].role == Role::Text;
            prose |= takes_prose;
        }
        if !captured {
            let (parent, _) = self.fit_parent(parent, level);
            match self.push(Node::new(Role::Text, parent)) {
                Ok(at) => self.append(at, &words, verbatim),
                Err(DocError::Nodes | DocError::Depth) => self.mark_truncated(),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn fit_parent(&mut self, parent: Option<usize>, level: usize) -> (Option<usize>, usize) {
        if level >= self.limits.depth {
            self.mark_truncated();
            (
                parent.and_then(|at| self.doc.nodes[at].parent),
                level.saturating_sub(1),
            )
        } else {
            (parent, level)
        }
    }

    fn walk(&mut self, body: NodeRef<'_>, base: &Url) -> Result<(), DocError> {
        let mut pending = vec![(body, None, 0, false)];
        while let Some((n, parent, level, verbatim)) = pending.pop() {
            if let Some(next) = n.sibling() {
                pending.push((next, parent, level, verbatim));
            }
            if n.is_text() {
                self.capture_text(parent, n.text(), verbatim, level)?;
                continue;
            }
            if self.doc.nodes.len() >= self.limits.nodes || self.limits.depth == 0 {
                self.mark_truncated();
                continue;
            }
            if hidden(&n)
                || matches!(
                    n.tag(),
                    "script" | "style" | "template" | "noscript" | "svg" | "head"
                )
            {
                continue;
            }
            if n.loss() {
                self.mark_truncated()
            }
            let in_sink =
                std::iter::successors(parent, |&at| self.doc.nodes[at].parent).any(|at| {
                    matches!(
                        self.doc.nodes[at].role,
                        Role::Heading(_)
                            | Role::Link
                            | Role::Button
                            | Role::Paragraph
                            | Role::ListItem
                            | Role::Document
                            | Role::CodeBlock
                    )
                });
            let role =
                classify(&n, n.tag(), in_sink).filter(|r| !(verbatim && *r == Role::CodeBlock));
            let (parent, level) = if let Some(role) = role {
                let (parent, level) = self.fit_parent(parent, level);
                let mut node = Node::new(role, parent);
                node.sponsored = ["data-layout", "data-type", "data-testid"]
                    .iter()
                    .any(|attr| n.attr(attr).is_some_and(|v| v.eq_ignore_ascii_case("ad")))
                    || n.attr("rel").is_some_and(|v| {
                        v.split_ascii_whitespace()
                            .any(|w| w.eq_ignore_ascii_case("sponsored"))
                    })
                    || n.attr("class")
                        .is_some_and(|v| v.split_ascii_whitespace().any(|w| w == "b_ad"));

                if role == Role::Link {
                    node.destination = self.resolve(&n, "href", base);
                }
                if role == Role::Image {
                    node.image_source = self.resolve(&n, "src", base);
                }
                let name = n
                    .attr("aria-label")
                    .filter(|v| !v.is_empty())
                    .or_else(|| (role == Role::Image).then(|| n.attr("alt")).flatten());
                if let Some(name) = name {
                    node.name = self.words(name);
                }
                if let Some(id) = n.attr("id") {
                    if !n.clipped("id")
                        && id.len()
                            <= self
                                .limits
                                .node_text_bytes
                                .min(self.limits.text_bytes - self.text_bytes)
                                - node.name.len()
                    {
                        node.element_id = Some(id.to_string());
                    } else {
                        self.mark_truncated();
                    }
                }
                (Some(self.push(node)?), level + 1)
            } else {
                (parent, level)
            };
            if let Some(child) = n.children().next() {
                pending.push((
                    child,
                    parent,
                    level,
                    verbatim || matches!(n.tag(), "pre" | "code"),
                ));
            }
        }
        Ok(())
    }
}

fn classify(n: &NodeRef, tag: &str, in_sink: bool) -> Option<Role> {
    use super::Landmark as L;
    use Role::*;
    if let Some(raw) = n.attr("role") {
        for token in raw.to_ascii_lowercase().split_ascii_whitespace() {
            return Some(match token {
                "none" | "presentation" => return None,
                "group" | "figure" => Group,
                "document" | "article" => Document,
                "heading" => Heading(Some(
                    n.attr("aria-level")
                        .and_then(|v| v.parse().ok())
                        .filter(|v| (1..=6).contains(v))
                        .unwrap_or(2),
                )),
                "main" => Landmark(Some(L::Main)),
                "navigation" => Landmark(Some(L::Navigation)),
                "banner" => Landmark(Some(L::Banner)),
                "contentinfo" => Landmark(Some(L::Contentinfo)),
                "form" => Landmark(Some(L::Form)),
                "complementary" => Landmark(Some(L::Complementary)),
                "region" => Landmark(Some(L::Region)),
                "search" => Landmark(Some(L::Search)),
                "paragraph" => Paragraph,
                "link" => Link,
                "button" => Button,
                "dialog" | "alertdialog" => Dialog,
                "list" | "feed" => List,
                "listitem" => ListItem,
                "img" => Image,
                "table" | "grid" => Table,
                "row" => Row,
                "cell" | "gridcell" => Cell,
                "rowheader" | "columnheader" => CellHeader,
                "textbox" => Textbox,
                "searchbox" => Searchbox,
                "checkbox" => Checkbox,
                "radio" => Radio,
                "combobox" => Combobox,
                "listbox" => Listbox,
                "option" => Role::Option,
                "spinbutton" => Spinbutton,
                "slider" => Slider,
                "tab" => Tab,
                "menuitem" => MenuItem,
                "progressbar" => Progress,
                "status" | "alert" => Status,
                _ => continue,
            });
        }
    }
    Some(match tag {
        "span" | "small"
            if n.children()
                .any(|child| child.is_text() && !child.text().trim().is_empty()) =>
        {
            Text
        }
        "body" | "div"
            if !in_sink
                && n.children()
                    .take(GENERIC_TEXT_CHILDREN)
                    .any(|c| c.is_text() && !c.text().trim().is_empty()) =>
        {
            Paragraph
        }
        "body" | "div" | "fieldset" | "details" | "figure" => Group,
        "article" => Document,
        "main" => Landmark(Some(L::Main)),
        "nav" => Landmark(Some(L::Navigation)),
        "header" => Landmark(Some(L::Banner)),
        "footer" => Landmark(Some(L::Contentinfo)),
        "aside" => Landmark(Some(L::Complementary)),
        "search" => Landmark(Some(L::Search)),
        "section" | "form"
            if ["aria-label", "aria-labelledby", "title"]
                .iter()
                .any(|k| n.has_attr(k)) =>
        {
            Landmark(Some(if tag == "section" { L::Region } else { L::Form }))
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Heading(tag[1..].parse().ok()),
        "p" | "blockquote" | "dt" | "dd" => Paragraph,
        "pre" | "code" => CodeBlock,
        "a" if n.has_attr("href") => Link,
        "img" => Image,
        "button" | "summary" => Button,
        "input" => match n
            .attr("type")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "search" => Searchbox,
            "submit" | "button" | "reset" => Button,
            "checkbox" => Checkbox,
            "radio" => Radio,
            "password" => Password,
            "number" => Spinbutton,
            "range" => Slider,
            _ => Textbox,
        },
        "textarea" => Textbox,
        "select" => Combobox,
        "option" => Role::Option,
        "ul" | "ol" | "menu" => List,
        "li" => ListItem,
        "dialog" => Dialog,
        "table" => Table,
        "tr" => Row,
        "td" => Cell,
        "th" => CellHeader,
        "progress" => Progress,
        "iframe" => FrameBoundary,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
