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
    pub input_bytes: usize,
    pub arena_elements: usize,
    pub arena_depth: usize,
    pub arena_text_bytes: usize,
    pub nodes: usize,
    pub depth: usize,
    pub text_bytes: usize,
    pub node_text_bytes: usize,
    pub url_bytes: usize,
}
impl Default for DocLimits {
    fn default() -> Self {
        Self {
            input_bytes: MAX_INPUT_BYTES,
            arena_elements: MAX_ARENA_ELEMENTS,
            arena_depth: MAX_ARENA_DEPTH,
            arena_text_bytes: MAX_ARENA_TEXT_BYTES,
            nodes: MAX_NODES,
            depth: MAX_DEPTH,
            text_bytes: MAX_TEXT_BYTES,
            node_text_bytes: MAX_NODE_TEXT_BYTES,
            url_bytes: MAX_URL_BYTES,
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
        url_bytes: MAX_URL_BYTES,
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
        url_bytes: MAX_URL_BYTES,
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
        if self.input_bytes > MAX_INPUT_BYTES
            || self.arena_elements > MAX_ARENA_ELEMENTS
            || self.arena_depth > MAX_ARENA_DEPTH
            || self.arena_text_bytes > MAX_ARENA_TEXT_BYTES
            || self.nodes > MAX_NODES
            || self.depth > MAX_DEPTH
            || self.text_bytes > MAX_TEXT_BYTES
            || self.node_text_bytes > MAX_NODE_TEXT_BYTES
            || self.url_bytes > MAX_URL_BYTES
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
    Url(#[from] KeyError),
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
    Landmark(Option<Landmark>),
    Heading(Option<u8>),
    Paragraph,
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

pub struct Node {
    pub role: Role,
    pub parent: Option<usize>,
    pub name: String,
    pub text: String,
    pub destination: Option<Url>,
    pub image_source: Option<Url>,
    pub hidden: bool,
    pub sponsored: bool,
    pub element_id: Option<String>,
}
impl Node {
    pub fn accessible_name(&self) -> &str {
        if self.name.is_empty() && matches!(self.role, Role::Heading(_) | Role::Link | Role::Button)
        {
            &self.text
        } else {
            &self.name
        }
    }
    pub fn new(role: Role, parent: Option<usize>) -> Self {
        Self {
            role,
            parent,
            name: String::new(),
            text: String::new(),
            destination: None,
            image_source: None,
            hidden: false,
            sponsored: false,
            element_id: None,
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
}
impl DocBuilder {
    pub fn new(page: &Url, limits: DocLimits) -> Result<Self, DocError> {
        let limits = limits.validate()?;
        check_url(page, limits.url_bytes)?;
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
        })
    }

    pub fn mark_truncated(&mut self) {
        self.doc.truncated = true;
    }

    pub fn push(&mut self, node: Node) -> Result<usize, DocError> {
        let index = self.doc.nodes.len();
        if index >= self.limits.nodes {
            return Err(DocError::Nodes);
        }
        let level = match node.parent {
            None => 0,
            Some(parent) => {
                self.path
                    .iter()
                    .rposition(|&at| at == parent)
                    .ok_or(DocError::Parent)?
                    + 1
            }
        };
        if level >= self.limits.depth {
            return Err(DocError::Depth);
        }
        if matches!(node.role, Role::Heading(Some(n)) if !(1..=6).contains(&n))
            || node.destination.is_some() && node.role != Role::Link
            || node.image_source.is_some() && node.role != Role::Image
        {
            return Err(DocError::Role);
        }
        for url in [&node.destination, &node.image_source]
            .into_iter()
            .flatten()
        {
            check_url(url, self.limits.url_bytes)?;
        }
        let size = node
            .name
            .len()
            .saturating_add(node.text.len())
            .saturating_add(node.element_id.as_deref().map_or(0, str::len));
        if size > self.limits.node_text_bytes || size > self.limits.text_bytes - self.text_bytes {
            return Err(DocError::Text);
        }
        self.text_bytes += size;
        self.path.truncate(level);
        self.path.push(index);
        self.doc.nodes.push(node);
        Ok(index)
    }

    pub fn finish(self) -> Doc {
        self.doc
    }
}
