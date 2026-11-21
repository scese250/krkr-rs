//! The expression tree the generator walks.
//!
//! A node is an opcode, a place in the script, a value and a list of
//! children. The generator walks the tree and emits the intermediate
//! code; the tree itself is the parser's, and the parser builds it with
//! the shape the generator walks.
//!
//! The nodes are held behind a shared handle, so the tree is a graph of
//! shared nodes rather than a tree of owned ones.
//!
//! A node holds its children in itself: a node has at most three
//! children, and the node that has more — the inline array and the
//! inline dictionary — keeps them in a buffer beside it. The children
//! of a node therefore cost no allocation of their own, and the walk the
//! generator makes over the tree touches one place in memory per node.

use std::rc::Rc;

use super::value::Value;

/// The children of a node.
///
/// The children a node holds itself are the three slots of the node; the
/// children beyond the third are in a buffer, and the node that has them
/// keeps the buffer.
#[derive(Clone, Debug)]
pub enum Children {
    /// The children the node holds itself.
    Held {
        /// The children, in the order they were added.
        children: [Option<Rc<Node>>; 3],
        /// The number of children the node holds.
        len: usize,
    },
    /// The children of a node that holds more than three.
    Buffered(Vec<Option<Rc<Node>>>),
}

impl Children {
    /// The children of a node that holds none yet.
    pub fn new() -> Self {
        Self::Held {
            children: [None, None, None],
            len: 0,
        }
    }

    /// The number of children the node holds.
    pub fn len(&self) -> usize {
        match self {
            Self::Held { len, .. } => *len,
            Self::Buffered(children) => children.len(),
        }
    }

    /// Whether the node holds no child.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The child at the given place.
    pub fn get(&self, index: usize) -> Option<&Rc<Node>> {
        match self {
            Self::Held { children, len } => {
                if index < *len {
                    children[index].as_ref()
                } else {
                    None
                }
            }
            Self::Buffered(children) => children.get(index).and_then(Option::as_ref),
        }
    }

    /// Adds a child to the node.
    ///
    /// The node holds three children itself; the fourth one moves the
    /// children into a buffer, and the ones after it go into the buffer
    /// too.
    pub fn push(&mut self, child: Option<Rc<Node>>) {
        let buffered = match self {
            Self::Held { children, len } => {
                if *len < children.len() {
                    children[*len] = child;
                    *len += 1;
                    return;
                }
                let mut buffered: Vec<Option<Rc<Node>>> =
                    children.iter().take(*len).cloned().collect();
                buffered.push(child);
                buffered
            }
            Self::Buffered(children) => {
                children.push(child);
                return;
            }
        };
        *self = Self::Buffered(buffered);
    }
}

impl Default for Children {
    fn default() -> Self {
        Self::new()
    }
}

/// An expression node.
#[derive(Clone, Debug)]
pub struct Node {
    /// The opcode of the node.
    pub opcode: i32,
    /// The place of the node in the script, counted in code units.
    pub position: i32,
    /// The value of the node, when it has one.
    pub value: Option<Value>,
    /// The children of the node.
    pub children: Children,
}

impl Node {
    /// A node with an opcode and a place.
    pub fn new(opcode: i32, position: i32) -> Rc<Node> {
        Rc::new(Self {
            opcode,
            position,
            value: None,
            children: Children::new(),
        })
    }

    /// A node with an opcode, a place and a value.
    pub fn with_value(opcode: i32, position: i32, value: Value) -> Rc<Node> {
        Rc::new(Self {
            opcode,
            position,
            value: Some(value),
            children: Children::new(),
        })
    }

    /// The number of children the node has.
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Whether the node has no children.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// The child at the given place.
    pub fn child(&self, index: usize) -> Option<&Rc<Node>> {
        self.children.get(index)
    }

    /// The value of the node, when it has one.
    pub fn value(&self) -> Option<&Value> {
        self.value.as_ref()
    }

    /// Whether the node is a constant.
    pub fn is_const(&self) -> bool {
        self.opcode == super::token::T_CONSTVAL
    }
}

/// Adds a child to a node.
///
/// The node is shared, so the child is added through a copy-on-write
/// handle.
pub fn add_child(node: &mut Rc<Node>, child: Rc<Node>) {
    Rc::make_mut(node).children.push(Some(child));
}

/// Sets the value of a node.
pub fn set_value(node: &mut Rc<Node>, value: Value) {
    Rc::make_mut(node).value = Some(value);
}

/// Sets the opcode of a node.
pub fn set_opcode(node: &mut Rc<Node>, opcode: i32) {
    Rc::make_mut(node).opcode = opcode;
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::vm::codegen::token;

    use super::{Node, add_child};

    /// A child of a node, named by its opcode.
    fn child(opcode: i32) -> Rc<Node> {
        Node::new(opcode, 0)
    }

    /// The children of a node keep their place.
    ///
    /// The reference adds a child to the node's own list of children, and
    /// the list holds the absent ones too: the empty call argument and
    /// the empty array element are children that are not there
    /// (`reference/cpp/core/tjs2/tjsInterCodeGen.cpp:116-121`).
    #[test]
    fn the_children_keep_their_place() {
        let mut node = Node::new(token::T_ARG, 0);
        assert!(node.is_empty(), "a node holds no child at first");

        Rc::make_mut(&mut node).children.push(None);
        add_child(&mut node, child(token::T_SYMBOL));

        assert_eq!(node.len(), 2, "the absent child is a place of its own");
        assert!(node.child(0).is_none(), "the absent child keeps its place");
        assert_eq!(
            node.child(1).map(|child| child.opcode),
            Some(token::T_SYMBOL),
            "the child after the absent one keeps its place"
        );
    }

    /// A node with more children than it holds itself keeps them all, in
    /// the order they were added.
    #[test]
    fn a_node_with_many_children_keeps_them_in_order() {
        let mut node = Node::new(token::T_INLINEARRAY, 0);
        let opcodes = [
            token::T_SYMBOL,
            token::T_THIS,
            token::T_GLOBAL,
            token::T_VOID,
            token::T_SUPER,
        ];
        for opcode in opcodes {
            add_child(&mut node, child(opcode));
        }
        assert_eq!(node.len(), opcodes.len(), "every child is kept");
        for (index, opcode) in opcodes.into_iter().enumerate() {
            assert_eq!(
                node.child(index).map(|child| child.opcode),
                Some(opcode),
                "the child at {index} is the one that was added there"
            );
        }
    }
}
