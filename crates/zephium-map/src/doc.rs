use crate::key::{check_url, KeyError, MAX_URL_BYTES};
use thiserror::Error;
use url::Url;

mod html;

pub const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ARENA_ELEMENTS: usize = 131_072;
pub const MAX_ARENA_DEPTH: usize = 128;
pub const MAX_ARENA_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_NODES: usize = 16_384;
pub const MAX_DEPTH: usize = 64;
pub const MAX_TEXT_BYTES: usize = 512 * 1024;
pub const MAX_NODE_TEXT_BYTES: usize = 8192;
pub const MAX_HEAD_ITEMS: usize = 16;

#[derive(Clone, Copy)]
pub struct DocLimits {
    pub input_bytes: u32,
    pub arena_elements: u32,
    pub arena_depth: u32,
    pub arena_text_bytes: u32,
    pub nodes: u32,
    pub depth: u32,
    pub text_bytes: u32,
    pub node_text_bytes: u32,
    pub url_bytes: u32,
}
impl Default for DocLimits {
    fn default() -> Self {
        Self {
            input_bytes: MAX_INPUT_BYTES as u32,
            arena_elements: MAX_ARENA_ELEMENTS as u32,
            arena_depth: MAX_ARENA_DEPTH as u32,
            arena_text_bytes: MAX_ARENA_TEXT_BYTES as u32,
            nodes: MAX_NODES as u32,
            depth: MAX_DEPTH as u32,
            text_bytes: MAX_TEXT_BYTES as u32,
            node_text_bytes: MAX_NODE_TEXT_BYTES as u32,
            url_bytes: MAX_URL_BYTES as u32,
        }
    }
}
impl DocLimits {
    pub const RESULTS: Self = Self {
        input_bytes: 2 * 1024 * 1024,
        arena_elements: 64 * 1024,
        arena_depth: 96,
        arena_text_bytes: 512 * 1024,
        nodes: 8192,
        depth: 48,
        text_bytes: 192 * 1024,
        node_text_bytes: 4096,
        url_bytes: MAX_URL_BYTES as u32,
    };
    pub const SITE_PAGE: Self = Self {
        input_bytes: 7 * 512 * 1024,
        arena_elements: 96 * 1024,
        arena_depth: 112,
        arena_text_bytes: 768 * 1024,
        nodes: 12 * 1024,
        depth: 56,
        text_bytes: 320 * 1024,
        node_text_bytes: 6144,
        url_bytes: MAX_URL_BYTES as u32,
    };
    pub const ARTICLE: Self = Self {
        arena_elements: 80 * 1024,
        nodes: 10 * 1024,
        arena_text_bytes: 896 * 1024,
        text_bytes: 448 * 1024,
        node_text_bytes: 7168,
        ..Self::SITE_PAGE
    };

    fn validate(self) -> Result<Self, DocError> {
        if self.input_bytes > MAX_INPUT_BYTES as u32
            || self.arena_elements > MAX_ARENA_ELEMENTS as u32
            || self.arena_depth > MAX_ARENA_DEPTH as u32
            || self.arena_text_bytes > MAX_ARENA_TEXT_BYTES as u32
            || self.nodes > MAX_NODES as u32
            || self.depth > MAX_DEPTH as u32
            || self.text_bytes > MAX_TEXT_BYTES as u32
            || self.node_text_bytes > MAX_NODE_TEXT_BYTES as u32
            || self.url_bytes > MAX_URL_BYTES as u32
        {
            Err(DocError::Limits)
        } else {
            Ok(self)
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DocError {
    #[error("limits exceed hard ceiling")]
    Limits,
    #[error("input exceeds parser budget")]
    Input,
    #[error("invalid document order or parent")]
    Parent,
    #[error("node count exceeds limit")]
    Nodes,
    #[error("depth exceeds limit")]
    Depth,
    #[error("text exceeds limit")]
    Text,
    #[error("invalid field for node role")]
    Role,
    #[error(transparent)]
    Url {
        #[from]
        error: KeyError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landmark {
    Main,
    Navigation,
    Banner,
    Contentinfo,
    Complementary,
    Search,
    Region,
    Form,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Document,
    Group,
    Landmark { landmark: Option<Landmark> },
    Heading { level: Option<u8> },
    Paragraph,
    Term,
    Definition,
    Text,
    CodeBlock,
    Link,
    Image,
    List,
    ListItem,
    Table,
    Row,
    Cell,
    CellHeader,
    Button,
    Textbox,
    Password,
    Searchbox,
    Checkbox,
    Radio,
    Combobox,
    Listbox,
    Option,
    Spinbutton,
    Slider,
    Tab,
    MenuItem,
    Dialog,
    Progress,
    Status,
    FrameBoundary,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Marks(u8);
impl Marks {
    pub const SPONSORED: Self = Self(1);
    pub const ACCEPTED: Self = Self(2);
    pub const SUGGESTED: Self = Self(4);
    pub const GENERATED: Self = Self(8);
    pub const FEATURED: Self = Self(16);
    pub const WIDGET: Self = Self(32);
    pub(crate) const ANCILLARY: Self = Self(64);
    pub(crate) const PROSE: Self = Self(128);
    pub fn has(self, mark: Self) -> bool {
        self.0 & mark.0 != 0
    }
    pub fn insert(&mut self, mark: Self) {
        self.0 |= mark.0;
    }
}
pub struct Node {
    pub role: Role,
    pub parent: Option<u32>,
    pub name: String,
    pub text: String,
    pub destination: Option<Url>,
    pub image_source: Option<Url>,
    pub hidden: bool,
    pub marks: Marks,
    pub language: Option<String>,
    pub element_id: Option<String>,
    pub element_id_cleaned: bool,
}
impl Node {
    pub(crate) fn parent_index(&self) -> Option<usize> {
        self.parent.map(|i| i as usize)
    }
    pub(crate) fn selector_id(&self) -> Option<&str> {
        self.element_id
            .as_deref()
            .filter(|_| !self.element_id_cleaned)
    }
    pub fn accessible_name(&self) -> &str {
        if self.name.is_empty()
            && matches!(
                self.role,
                Role::Heading { level: _ } | Role::Link | Role::Button
            )
        {
            &self.text
        } else {
            &self.name
        }
    }
    pub fn new(role: Role, parent: Option<u32>) -> Self {
        Self {
            role,
            parent,
            name: String::new(),
            text: String::new(),
            destination: None,
            image_source: None,
            hidden: false,
            marks: Marks::default(),
            language: None,
            element_id: None,
            element_id_cleaned: false,
        }
    }
}

#[derive(Default)]
pub struct Head {
    pub truncated: bool,
    pub title: String,
    pub canonical: Option<Url>,
    pub site_name: String,
    pub alternates: Vec<Alternate>,
    pub linked_data: Vec<String>,
}
pub struct Alternate {
    pub url: Url,
    pub media_type: String,
    pub language: String,
    pub title: String,
}

pub struct Doc {
    page: Url,
    nodes: Vec<Node>,
    head: Head,
    truncated: bool,
}
impl Doc {
    pub fn page(&self) -> &Url {
        &self.page
    }
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
    pub fn head(&self) -> &Head {
        &self.head
    }
    pub fn truncated(&self) -> bool {
        self.head.truncated || self.body_truncated()
    }
    pub fn body_truncated(&self) -> bool {
        self.truncated
    }
}

pub struct DocBuilder {
    doc: Doc,
    limits: DocLimits,
    text_bytes: usize,
    path: Vec<usize>,
    gap: bool,
}
impl DocBuilder {
    pub fn new(page: &Url, limits: DocLimits) -> Result<Self, DocError> {
        let limits = limits.validate()?;
        check_url(page, limits.url_bytes as usize)?;
        Ok(Self {
            doc: Doc {
                page: page.clone(),
                nodes: Vec::new(),
                head: Head::default(),
                truncated: false,
            },
            limits,
            text_bytes: 0,
            path: Vec::new(),
            gap: false,
        })
    }

    pub fn mark_truncated(&mut self) {
        self.doc.truncated = true;
    }

    pub fn push(&mut self, node: Node) -> Result<u32, DocError> {
        let index = self.doc.nodes.len();
        if index >= self.limits.nodes as usize {
            return Err(DocError::Nodes);
        }
        let level = match node.parent_index() {
            None => 0,
            Some(parent) => {
                self.path
                    .iter()
                    .rposition(|&at| at == parent)
                    .ok_or(DocError::Parent)?
                    + 1
            }
        };
        if level >= self.limits.depth as usize {
            return Err(DocError::Depth);
        }
        if matches!(node.role, Role::Heading{level:Some(n)} if !(1..=6).contains(&n))
            || node.destination.is_some() && node.role != Role::Link
            || node.image_source.is_some() && node.role != Role::Image
            || node
                .language
                .as_ref()
                .is_some_and(|l| node.role != Role::CodeBlock || !valid_language(l))
        {
            return Err(DocError::Role);
        }
        for url in [&node.destination, &node.image_source]
            .into_iter()
            .flatten()
        {
            check_url(url, self.limits.url_bytes as usize)?;
        }
        let size = node
            .name
            .len()
            .saturating_add(node.text.len())
            .saturating_add(node.language.as_deref().map_or(0, str::len))
            .saturating_add(node.element_id.as_deref().map_or(0, str::len));
        if size > self.limits.node_text_bytes as usize
            || size > self.limits.text_bytes as usize - self.text_bytes
        {
            return Err(DocError::Text);
        }
        self.text_bytes += size;
        self.path.truncate(level);
        self.path.push(index);
        self.doc.nodes.push(node);
        Ok(index as u32)
    }

    pub fn finish(mut self) -> Doc {
        for n in &mut self.doc.nodes {
            n.text.retain(crate::visible_char);
            n.name.retain(crate::visible_char);
            if let Some(id) = &mut n.element_id {
                let raw = id.len();
                id.retain(crate::visible_char);
                n.element_id_cleaned = id.len() != raw;
            }
        }
        self.doc.head.title.retain(crate::visible_char);
        self.doc.head.site_name.retain(crate::visible_char);
        for a in &mut self.doc.head.alternates {
            a.title.retain(crate::visible_char);
        }
        crate::linked::mark_answers(&mut self.doc.nodes, &self.doc.head);
        self.doc
    }
}

impl Marks {
    pub(crate) fn from_label(value: &str) -> Option<Self> {
        let v = value.trim().to_ascii_lowercase();
        Some(match v.rsplit('/').next()? {
            "acceptedanswer" | "accepted-answer" => Self::ACCEPTED,
            "suggestedanswer" | "suggested-answer" => Self::SUGGESTED,
            "sponsored" | "sponsoredcontent" => Self::SPONSORED,
            "ai overview"
            | "ai overviews"
            | "ai-overview"
            | "generated overview"
            | "ai-generated answer"
            | "ai-generated overview"
            | "ai answer"
            | "ai-answer"
            | "generative answer" => Self::GENERATED,
            "featured snippet" | "featured-snippet" | "b_ansfs" => Self::FEATURED,
            "calculator" | "converter" | "currency converter" | "unit converter" | "weather"
            | "weather card" | "translation" | "translator" | "map pack" | "local pack"
            | "clock" => Self::WIDGET,
            _ => return None,
        })
    }
}

pub const MAX_LANGUAGE_BYTES: usize = 32;
fn valid_language(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_LANGUAGE_BYTES
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+#_-".contains(&b))
}
pub use html::MAX_CHARSET_LABEL_BYTES;
