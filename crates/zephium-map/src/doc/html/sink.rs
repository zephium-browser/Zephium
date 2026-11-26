use super::arena::{Arena, Id, IGNORED};
use html5ever::{
    local_name, ns,
    tendril::StrTendril,
    tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink},
    Attribute, QualName,
};
use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
};

#[derive(Clone)]
pub(super) struct Handle {
    id: Id,
    name: QualName,
    overflow_id: u32,
    pub(super) markup_bytes: usize,
    integration: bool,
    fragment: bool,
}
impl Handle {
    fn plain(id: Id) -> Self {
        Self {
            id,
            name: QualName::new(None, ns!(html), local_name!("")),
            overflow_id: 0,
            markup_bytes: 0,
            integration: false,
            fragment: false,
        }
    }
}

pub(super) struct Sink {
    pub(super) arena: RefCell<Arena>,
    pub(super) received_text: Cell<bool>,
    overflow_ids: Cell<u32>,
}

impl Sink {
    pub(super) fn new(arena: Arena) -> Self {
        Self {
            arena: RefCell::new(arena),
            received_text: Cell::default(),
            overflow_ids: Cell::default(),
        }
    }

    fn insert(&self, parent: Id, before: Id, child: NodeOrText<Handle>) {
        let mut arena = self.arena.borrow_mut();
        match child {
            NodeOrText::AppendNode(h) => arena.insert(h.id, parent, before),
            NodeOrText::AppendText(text) => {
                self.received_text.set(true);
                arena.text(parent, before, &text)
            }
        }
    }
}
impl TreeSink for Sink {
    type Handle = Handle;
    type Output = Arena;
    type ElemName<'a> = &'a QualName;

    fn finish(self) -> Arena {
        self.arena.into_inner()
    }
    fn parse_error(&self, _: Cow<'static, str>) {}
    fn get_document(&self) -> Handle {
        Handle::plain(0)
    }
    fn elem_name<'a>(&'a self, h: &'a Handle) -> &'a QualName {
        &h.name
    }
    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        // html5ever can keep the original tag (including discarded attributes) for reconstruction.
        let markup_bytes = name.local.len()
            + std::mem::size_of_val(attrs.as_slice())
            + attrs
                .iter()
                .map(|a| a.name.local.len() + a.value.len())
                .sum::<usize>();
        let id = self.arena.borrow_mut().create(&name.local, attrs);
        // A single tree-builder step can create several implied elements after the last slot.
        let overflow_id = if id == IGNORED {
            let next = self.overflow_ids.get() + 1;
            self.overflow_ids.set(next);
            next
        } else {
            0
        };
        Handle {
            id,
            name,
            overflow_id,
            markup_bytes,
            integration: flags.mathml_annotation_xml_integration_point,
            fragment: false,
        }
    }
    fn create_comment(&self, _: StrTendril) -> Handle {
        Handle::plain(IGNORED)
    }
    fn create_pi(&self, _: StrTendril, _: StrTendril) -> Handle {
        Handle::plain(IGNORED)
    }
    fn append(&self, p: &Handle, c: NodeOrText<Handle>) {
        self.insert(p.id, 0, c)
    }
    fn append_based_on_parent_node(&self, element: &Handle, prev: &Handle, c: NodeOrText<Handle>) {
        let parent = self.arena.borrow().attached_parent(element.id);
        if let Some(p) = parent {
            self.insert(p, element.id, c)
        } else {
            self.append(prev, c)
        }
    }
    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}
    fn get_template_contents(&self, target: &Handle) -> Handle {
        // The ignored template subtree owns the fragment's child list.
        Handle {
            fragment: true,
            ..target.clone()
        }
    }
    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        x.id == y.id && x.overflow_id == y.overflow_id && x.fragment == y.fragment
    }
    fn set_quirks_mode(&self, _: QuirksMode) {}
    fn append_before_sibling(&self, s: &Handle, c: NodeOrText<Handle>) {
        let parent = self.arena.borrow().attached_parent(s.id);
        if let Some(p) = parent {
            self.insert(p, s.id, c)
        }
    }
    fn add_attrs_if_missing(&self, h: &Handle, attrs: Vec<Attribute>) {
        self.arena.borrow_mut().add_attrs(h.id, attrs)
    }
    fn remove_from_parent(&self, h: &Handle) {
        if h.id != IGNORED && !self.arena.borrow().stopped {
            self.arena.borrow_mut().detach(h.id)
        }
    }
    fn reparent_children(&self, h: &Handle, p: &Handle) {
        self.arena.borrow_mut().move_children(h.id, p.id);
    }
    fn is_mathml_annotation_xml_integration_point(&self, h: &Handle) -> bool {
        h.integration
    }
}
