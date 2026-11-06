//! The scanned directories as a tree: counting hard links, adding subtrees
//! into their parents, and visiting the rows in the order of their paths.

use super::{HardLink, Node, OwnerUsage, Totals};
use crate::error::Result;
use crate::traverse::{HardLinks, LargestFiles};

/// The parent ID of the root.
pub(super) const NO_PARENT: u32 = u32::MAX;
const UNREAD: u32 = u32::MAX;

/// The directories that were read, by ID, and where each ID sits.
pub(super) struct Tree {
    /// Sorted by ID; the root is first.
    nodes: Vec<Node>,
    /// The index of each ID in `nodes`, or `UNREAD`.
    position: Vec<u32>,
}

impl Tree {
    /// Place the nodes by ID; `None` if the root was not read.
    pub fn from_nodes(mut nodes: Vec<Node>, ids: u32) -> Option<Self> {
        // Sort in place: a sorted copy would hold every node twice.
        nodes.sort_unstable_by_key(|node| node.id);
        let mut position = vec![UNREAD; ids as usize];
        for (index, node) in nodes.iter().enumerate() {
            position[node.id as usize] = index as u32;
        }
        let tree = Tree { nodes, position };
        (tree.index_of(0) == Some(0)).then_some(tree)
    }

    fn index_of(&self, id: u32) -> Option<usize> {
        match self.position.get(id as usize) {
            Some(&index) if index != UNREAD => Some(index as usize),
            _ => None,
        }
    }

    pub fn root(&self) -> &Node {
        &self.nodes[0]
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Count each hard-linked file once, in the directory of the link whose
    /// path sorts first, so that repeated scans charge it to the same place
    /// whichever thread saw which link.
    pub fn count_hard_links(
        &mut self,
        links: HardLinks<HardLink>,
        large_files: &LargestFiles,
        collect_users: bool,
    ) {
        for link in links.into_links() {
            let file = link.file;
            if let Some(index) = self.index_of(link.directory) {
                let node = &mut self.nodes[index];
                node.totals
                    .add(&Totals::file(file.allocated, file.apparent, file.modified));
                if collect_users {
                    node.owners
                        .add(file.uid, &OwnerUsage::file(file.allocated, file.apparent));
                }
            }
            if large_files.wants(file.allocated) {
                large_files.offer(file);
            }
        }
    }

    /// Turn each directory's own usage into the usage of its subtree.
    ///
    /// Children have higher IDs than their parents, so one pass from the
    /// highest ID down adds every subtree into its parent before the parent
    /// is added into its own.
    pub fn roll_up(&mut self, collect_users: bool, user_max_depth: u32) {
        for index in (1..self.nodes.len()).rev() {
            let Some(parent_index) = self.index_of(self.nodes[index].parent) else {
                continue;
            };
            let (lower, upper) = self.nodes.split_at_mut(index);
            let (node, parent) = (&mut upper[0], &mut lower[parent_index]);
            parent.totals.add(&node.totals);
            if collect_users && !node.owners.is_empty() {
                parent.owners.merge(&node.owners);
                // Below the owner depth no row needs the directory's own
                // list once its parent has it.
                if node.depth > user_max_depth {
                    node.owners = Default::default();
                }
            }
        }
    }

    /// The order of the rows below every directory; see [`PathOrder`].
    pub fn path_order(&self) -> PathOrder {
        PathOrder::new(self)
    }

    /// Visit the directories down to `max_depth` in the order of their
    /// stored paths, with each one's path.
    ///
    /// Rows inserted in primary-key order build a compact table without
    /// sorting every path first, and only the current path is held.
    pub fn visit_in_path_order(
        &self,
        order: &PathOrder,
        max_depth: Option<u32>,
        mut visit: impl FnMut(&Node, &str) -> Result<()>,
    ) -> Result<()> {
        let wanted = |depth: u32| max_depth.is_none_or(|limit| depth <= limit);
        // Each level holds its remaining items and the path length to
        // restore when it is done.
        let mut stack = vec![(order.below(0).iter(), 0usize)];
        let mut path = String::new();
        while let Some((items, _)) = stack.last_mut() {
            let Some(&item) = items.next() else {
                if let Some((_, length)) = stack.pop() {
                    path.truncate(length);
                }
                continue;
            };
            let (index, below) = Item::unpack(item);
            let node = &self.nodes[index as usize];
            let length = path.len();
            if below {
                if wanted(node.depth + 1) {
                    push_component(&mut path, &node.name);
                    stack.push((order.below(index).iter(), length));
                }
            } else if wanted(node.depth) {
                if index == 0 {
                    path.push('.');
                } else {
                    push_component(&mut path, &node.name);
                }
                visit(node, &path)?;
                path.truncate(length);
            }
        }
        Ok(())
    }
}

fn push_component(path: &mut String, name: &str) {
    if !path.is_empty() {
        path.push('/');
    }
    path.push_str(name);
}

/// A step of the walk in path order: the row of a directory, or the rows
/// below it. Packed into one number, with the low bit set for the rows
/// below.
struct Item;

impl Item {
    fn row(index: u32) -> u32 {
        index << 1
    }

    fn below(index: u32) -> u32 {
        (index << 1) | 1
    }

    fn unpack(item: u32) -> (u32, bool) {
        (item >> 1, item & 1 == 1)
    }
}

/// For each directory, its children's rows and the rows below them, sorted
/// as SQLite compares the stored paths: byte by byte, so a child's row sorts
/// by its name and the rows below it by its name followed by `/`. The
/// root's list also holds the root's own row, `.`.
///
/// The order is sorted once and serves every walk, one for the directories
/// and one per owner.
pub(super) struct PathOrder {
    start: Vec<u32>,
    items: Vec<u32>,
}

impl PathOrder {
    fn new(tree: &Tree) -> Self {
        let count = tree.nodes.len();
        let parent_of = |node: &Node| {
            (node.parent != NO_PARENT)
                .then(|| tree.index_of(node.parent))
                .flatten()
        };
        let mut children = vec![0u32; count];
        for node in &tree.nodes {
            if let Some(parent) = parent_of(node) {
                children[parent] += 1;
            }
        }
        let has_children = |index: usize| children[index] > 0;
        // A child takes one item for its row, and one for the rows below it
        // if it has children; the root takes one more for its own row.
        let mut start = vec![0u32; count + 1];
        start[1] = 1;
        for (index, node) in tree.nodes.iter().enumerate() {
            if let Some(parent) = parent_of(node) {
                start[parent + 1] += 1 + u32::from(has_children(index));
            }
        }
        for index in 0..count {
            start[index + 1] += start[index];
        }
        let mut items = vec![0u32; start[count] as usize];
        let mut next = start[..count].to_vec();
        let mut place = |parent: usize, item: u32| {
            items[next[parent] as usize] = item;
            next[parent] += 1;
        };
        place(0, Item::row(0));
        for (index, node) in tree.nodes.iter().enumerate() {
            if let Some(parent) = parent_of(node) {
                place(parent, Item::row(index as u32));
                if has_children(index) {
                    place(parent, Item::below(index as u32));
                }
            }
        }
        let mut order = PathOrder { start, items };
        order.sort(tree);
        order
    }

    fn sort(&mut self, tree: &Tree) {
        let name = |index: u32| -> &[u8] {
            if index == 0 {
                b"."
            } else {
                tree.nodes[index as usize].name.as_bytes()
            }
        };
        for parent in 0..tree.nodes.len() {
            let range = self.start[parent] as usize..self.start[parent + 1] as usize;
            self.items[range].sort_unstable_by(|&left, &right| {
                let (left, left_below) = Item::unpack(left);
                let (right, right_below) = Item::unpack(right);
                name(left)
                    .iter()
                    .chain(left_below.then_some(&b'/'))
                    .cmp(name(right).iter().chain(right_below.then_some(&b'/')))
            });
        }
    }

    /// The items below the directory at `index`.
    fn below(&self, index: u32) -> &[u32] {
        let index = index as usize;
        &self.items[self.start[index] as usize..self.start[index + 1] as usize]
    }
}
