use super::{DocLimits, Marks};
use html5ever::{local_name, ns, tendril::StrTendril, Attribute, LocalName};

// Zero is both the document handle and the null link. The document is never a child.
pub(super) type Id = u32;
pub(super) const IGNORED: Id = Id::MAX;
const TEXT: u8 = 1;
const OPAQUE: u8 = 2;
const NO_TEXT: u8 = 4;
const LOSS: u8 = 8;
const JSON: u8 = 16;
const HIDDEN_BY_LIFT: u8 = 32;
const ATTACHED: u8 = 64;
const ATTRIBUTE_NAMES: &[&str] = &[
    "href",
    "src",
    "role",
    "aria-level",
    "aria-hidden",
    "aria-label",
    "hidden",
    "id",
    "type",
    "rel",
    "name",
    "property",
    "content",
    "charset",
    "http-equiv",
    "title",
    "alt",
    "itemprop",
    "itemtype",
    "datetime",
    "class",
    "data-layout",
    "data-type",
    "data-testid",
    "aria-labelledby",
    "hreflang",
];
const ATTRIBUTES_PER_ELEMENT_BUDGET: usize = 4;

#[derive(Default)]
struct Record {
    tag: LocalName,
    parent: Id,
    first: Id,
    last: Id,
    prev: Id,
    next: Id,
    // Head of the attribute chain, or index into text runs.
    data: u32,
    depth: u8,
    flags: u8,
    lifted_marks: Marks,
}

struct Attr {
    next: u32,
    start: u32,
    len: u16,
    key: u8,
    clipped: bool,
}

// Grow by separate slabs so a large page never reallocates and copies its full record array.
const SLAB_ITEMS: usize = 512;
struct Slabs<T> {
    chunks: Vec<Vec<T>>,
    len: usize,
}
impl<T> Slabs<T> {
    fn new() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
        }
    }
    fn len(&self) -> usize {
        self.len
    }
    fn push(&mut self, item: T) {
        let slot = self.len / SLAB_ITEMS;
        if slot == self.chunks.len() {
            self.chunks.push(if slot == 0 {
                Vec::new()
            } else {
                Vec::with_capacity(SLAB_ITEMS)
            });
        }
        self.chunks[slot].push(item);
        self.len += 1;
    }
}
impl<T> std::ops::Index<usize> for Slabs<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        &self.chunks[i / SLAB_ITEMS][i % SLAB_ITEMS]
    }
}
impl<T> std::ops::IndexMut<usize> for Slabs<T> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        &mut self.chunks[i / SLAB_ITEMS][i % SLAB_ITEMS]
    }
}

pub(super) struct Arena {
    records: Slabs<Record>,
    attributes: Slabs<Attr>,
    values: String,
    runs: Slabs<StrTendril>,
    elements: usize,
    bytes: usize,
    limits: DocLimits,
    pub(super) saw_table: bool,
    pub(super) stopped: bool,
    pub(super) lifted: bool,
}

#[derive(Clone, Copy)]
pub(super) struct NodeRef<'a> {
    arena: &'a Arena,
    id: Id,
}

impl<'a> NodeRef<'a> {
    fn record(self) -> &'a Record {
        &self.arena.records[self.id as usize]
    }
    pub(super) fn tag(self) -> &'a str {
        &self.record().tag
    }
    pub(super) fn is_text(self) -> bool {
        self.record().flags & TEXT != 0
    }
    pub(super) fn text(self) -> &'a str {
        if self.is_text() {
            &self.arena.runs[self.record().data as usize]
        } else {
            ""
        }
    }
    pub(super) fn loss(self) -> bool {
        self.record().flags & LOSS != 0
    }
    pub(super) fn hidden_by_lift(self) -> bool {
        self.record().flags & HIDDEN_BY_LIFT != 0
    }
    pub(super) fn lifted_marks(self) -> Marks {
        self.record().lifted_marks
    }
    pub(super) fn sibling(self) -> Option<Self> {
        let next = self.record().next;
        (next != 0).then_some(self.arena.node(next))
    }
    pub(super) fn children(self) -> impl Iterator<Item = Self> {
        let first = self.record().first;
        std::iter::successors((first != 0).then_some(self.arena.node(first)), |n| {
            let next = n.record().next;
            (next != 0).then_some(self.arena.node(next))
        })
    }
    fn attribute(self, key: &str) -> Option<&'a Attr> {
        if self.is_text() {
            return None;
        }
        let mut i = self.record().data;
        while i != 0 {
            let attr = &self.arena.attributes[(i - 1) as usize];
            if ATTRIBUTE_NAMES[attr.key as usize] == key {
                return Some(attr);
            }
            i = attr.next;
        }
        None
    }
    pub(super) fn attr(self, key: &str) -> Option<&'a str> {
        let attr = self.attribute(key)?;
        let start = attr.start as usize;
        Some(&self.arena.values[start..start + usize::from(attr.len)])
    }
    pub(super) fn clipped(self, key: &str) -> bool {
        self.attribute(key).is_some_and(|a| a.clipped)
    }
    pub(super) fn has_attr(self, key: &str) -> bool {
        self.attribute(key).is_some()
    }
}

impl Arena {
    pub(super) fn new(limits: DocLimits) -> Self {
        let mut records = Slabs::new();
        records.push(Record::default());
        Self {
            records,
            attributes: Slabs::new(),
            values: String::new(),
            runs: Slabs::new(),
            bytes: 0,
            elements: 0,
            saw_table: false,
            stopped: false,
            lifted: false,
            limits,
        }
    }
    pub(super) fn node(&self, id: Id) -> NodeRef<'_> {
        NodeRef { arena: self, id }
    }
    pub(super) fn create(&mut self, tag: &LocalName, attrs: Vec<Attribute>) -> Id {
        if self.stopped || self.elements >= self.limits.arena_elements as usize {
            self.stopped = true;
            return IGNORED;
        }
        self.elements += 1;
        self.saw_table |= tag.as_ref() == "table";
        let id = self.records.len() as Id;
        // The tree builder's live handles keep exact names. Unknown names have no role.
        self.records.push(Record {
            tag: if !tag.is_dynamic() {
                tag.clone()
            } else {
                local_name!("")
            },
            flags: if matches!(
                tag.as_ref(),
                "script" | "style" | "template" | "svg" | "noscript"
            ) {
                OPAQUE
            } else {
                0
            },
            ..Default::default()
        });
        self.add_attrs(id, attrs);
        if tag.as_ref() == "script"
            && self
                .node(id)
                .attr("type")
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/ld+json"))
        {
            self.records[id as usize].flags = (self.records[id as usize].flags & !OPAQUE) | JSON;
        }
        id
    }
    pub(super) fn add_attrs(&mut self, id: Id, attrs: Vec<Attribute>) {
        if self.stopped || id == IGNORED {
            return;
        }
        for attr in attrs {
            if attr.name.ns != ns!() {
                continue;
            }
            let name = attr.name.local.as_ref();
            let Some(key) = ATTRIBUTE_NAMES.iter().position(|&k| k == name) else {
                continue;
            };
            if name == "src"
                && attr
                    .value
                    .trim_start()
                    .get(..5)
                    .is_some_and(|s| s.eq_ignore_ascii_case("data:"))
            {
                continue;
            }
            if self.node(id).has_attr(name) {
                continue;
            }
            if self.attributes.len()
                >= self.limits.arena_elements as usize * ATTRIBUTES_PER_ELEMENT_BUDGET
            {
                self.stopped = true;
                break;
            }
            let cap = match name {
                "href" | "src" => self.limits.url_bytes as usize,
                _ => super::MAX_NODE_TEXT_BYTES,
            };
            let len = attr.value.floor_char_boundary(cap);
            if len > self.limits.arena_text_bytes as usize - self.bytes {
                self.stopped = true;
                break;
            }
            let clipped = len < attr.value.len();
            let n = &mut self.records[id as usize];
            if clipped {
                n.flags |= LOSS
            }
            self.attributes.push(Attr {
                next: n.data,
                start: self.values.len() as u32,
                len: len as u16,
                key: key as u8,
                clipped,
            });
            n.data = self.attributes.len() as u32;
            self.values.push_str(&attr.value[..len]);
            self.bytes += len;
        }
    }
    pub(super) fn detach(&mut self, id: Id) {
        let n = &self.records[id as usize];
        let (parent, prev, next) = (n.parent, n.prev, n.next);
        if prev != 0 {
            self.records[prev as usize].next = next
        } else if self.records[parent as usize].first == id {
            self.records[parent as usize].first = next
        }
        if next != 0 {
            self.records[next as usize].prev = prev
        } else if self.records[parent as usize].last == id {
            self.records[parent as usize].last = prev
        }
        let n = &mut self.records[id as usize];
        n.flags &= !ATTACHED;
        n.parent = 0;
        n.next = 0;
        n.prev = 0;
    }
    pub(super) fn parent(&self, id: Id) -> Id {
        self.records[id as usize].parent
    }
    pub(super) fn attached_parent(&self, id: Id) -> Option<Id> {
        (id != IGNORED && self.records[id as usize].flags & ATTACHED != 0).then(|| self.parent(id))
    }

    pub(super) fn move_children(&mut self, from: Id, to: Id) {
        if from == IGNORED || to == IGNORED {
            return;
        }
        let last = self.records[from as usize].last;
        let mut child = self.first(from);
        while child != 0 && !self.stopped {
            let next = self.records[child as usize].next;
            self.insert(child, to, 0);
            if child == last {
                break;
            }
            child = next;
        }
    }

    fn first(&self, id: Id) -> Id {
        self.records[id as usize].first
    }

    pub(super) fn insert(&mut self, id: Id, mut parent: Id, mut before: Id) {
        if self.stopped || id == IGNORED || parent == IGNORED {
            return;
        }
        if self.limits.arena_depth as usize == 0 {
            self.stopped = true;
            return;
        }
        let p = &self.records[parent as usize];
        let no_text = p.flags & (OPAQUE | NO_TEXT) != 0;
        let mut hidden = p.flags & HIDDEN_BY_LIFT != 0;
        let mut lifted_marks = p.lifted_marks;
        if usize::from(p.depth) >= self.limits.arena_depth as usize
            && self.records[id as usize].flags & TEXT == 0
        {
            self.lifted = true;
            let lifted = self.node(parent);
            hidden |= super::hidden(&lifted) || no_text;
            let marks = super::marks(&lifted);
            for mark in [Marks::GENERATED, Marks::SPONSORED] {
                if marks.has(mark) {
                    lifted_marks.insert(mark);
                }
            }
            parent = p.parent;
            before = 0;
        }
        if id == parent || id == before {
            self.stopped = true;
            return;
        }
        if self.first(id) != 0 {
            let mut ancestor = parent;
            while ancestor != 0 {
                if ancestor == id {
                    self.stopped = true;
                    return;
                }
                ancestor = self.parent(ancestor);
            }
        }
        self.detach(id);
        let prev = if before == 0 {
            self.records[parent as usize].last
        } else {
            self.records[before as usize].prev
        };
        if prev != 0 {
            self.records[prev as usize].next = id
        } else {
            self.records[parent as usize].first = id
        }
        if before != 0 {
            self.records[before as usize].prev = id
        } else {
            self.records[parent as usize].last = id
        }
        let depth = self.records[parent as usize].depth + 1;
        let n = &mut self.records[id as usize];
        n.parent = parent;
        n.prev = prev;
        n.next = before;
        n.depth = depth;
        n.flags = (n.flags & !(NO_TEXT | HIDDEN_BY_LIFT)) | ATTACHED;
        n.lifted_marks.insert(lifted_marks);
        if no_text {
            n.flags |= NO_TEXT
        }
        if hidden {
            n.flags |= HIDDEN_BY_LIFT
        }
        // Children may already exist during adoption-agency moves. Reapply depth and context.
        if n.first != 0 {
            self.move_children(id, id)
        }
    }
    pub(super) fn text(&mut self, parent: Id, before: Id, value: &str) {
        if self.stopped || parent == IGNORED || value.is_empty() {
            return;
        }
        let p = &self.records[parent as usize];
        if p.flags & (OPAQUE | NO_TEXT) != 0 {
            return;
        }
        let prev = if before == 0 {
            p.last
        } else {
            self.records[before as usize].prev
        };
        let existing = prev != 0 && self.node(prev).is_text();
        let mut length = value.len();
        if p.flags & JSON != 0 {
            let used = if existing {
                self.node(prev).text().len()
            } else {
                0
            };
            let json_room = (self.limits.node_text_bytes as usize).saturating_sub(used);
            if value.len() > json_room {
                self.records[parent as usize].flags |= LOSS
            }
            length = length.min(json_room);
        }
        let room = self.limits.arena_text_bytes as usize - self.bytes;
        let len = value.floor_char_boundary(room.min(length));
        if len == 0 {
            self.stopped |= length > room;
            return;
        }
        if existing {
            let run = self.records[prev as usize].data as usize;
            self.runs[run].push_slice(&value[..len]);
        } else {
            if self.runs.len() > self.limits.arena_elements as usize {
                self.stopped = true;
                return;
            }
            let id = self.records.len() as Id;
            self.records.push(Record {
                data: self.runs.len() as u32,
                flags: TEXT,
                ..Default::default()
            });
            self.runs.push(StrTendril::from_slice(&value[..len]));
            self.insert(id, parent, before);
        }
        self.bytes += len;
        self.stopped |= length > room;
    }
}

#[cfg(test)]
impl Arena {
    pub(super) fn element_count(&self) -> usize {
        self.elements
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use html5ever::QualName;

    #[test]
    fn record_layout_and_capacity() {
        assert_eq!(std::mem::size_of::<Record>(), 40);
        assert_eq!(std::mem::size_of::<Attr>(), 12);
        assert_eq!(std::mem::size_of::<StrTendril>(), 16);
    }

    #[test]
    fn metadata_and_text_slots_have_independent_budgets() {
        let limits = DocLimits {
            arena_elements: 1,
            ..Default::default()
        };
        let attr = |name: &str| Attribute {
            name: QualName::new(None, ns!(), LocalName::from(name)),
            value: "".into(),
        };
        let mut arena = Arena::new(limits);
        let id = arena.create(
            &local_name!("p"),
            ATTRIBUTE_NAMES[..ATTRIBUTES_PER_ELEMENT_BUDGET]
                .iter()
                .map(|name| attr(name))
                .collect(),
        );
        assert!(!arena.stopped);
        arena.add_attrs(
            id,
            vec![attr(ATTRIBUTE_NAMES[ATTRIBUTES_PER_ELEMENT_BUDGET])],
        );
        assert!(arena.stopped);
        assert_eq!(arena.attributes.len(), ATTRIBUTES_PER_ELEMENT_BUDGET);

        let mut arena = Arena::new(limits);
        let id = arena.create(&local_name!("div"), Vec::new());
        for _ in 0..=limits.arena_elements as usize {
            arena.text(id, 0, "t");
            assert!(!arena.stopped);
            arena.detach(arena.first(id));
        }
        arena.text(id, 0, "t");
        assert!(arena.stopped);
        assert_eq!(arena.runs.len(), limits.arena_elements as usize + 1);
    }

    #[test]
    fn large_retained_and_discarded_attributes() {
        let huge = "a".repeat(1024 * 1024);
        let attr = |name: &str| Attribute {
            name: QualName::new(None, ns!(), LocalName::from(name)),
            value: huge.as_str().into(),
        };
        let mut arena = Arena::new(DocLimits::default());
        let id = arena.create(
            &local_name!("p"),
            vec![attr("data-unused"), attr("aria-label")],
        );
        assert_eq!(arena.attributes.len(), 1);
        assert_eq!(arena.values.len(), super::super::MAX_NODE_TEXT_BYTES);
        assert!(arena.node(id).clipped("aria-label"));
        assert!(!arena.node(id).has_attr("data-unused"));
        for name in ["aria-label", "data-unused"] {
            let html = format!("<p>before</p><p {name}='{huge}'>after");
            let arena = super::super::decode::parse(html.as_bytes(), None, Default::default());
            assert!(arena.stopped);
            assert!(arena.values.len() <= super::super::MAX_NODE_TEXT_BYTES);
        }
    }
}
