//! The on-disk format: the container and the automaton blocks inside it.

pub mod automaton;
pub mod container;

pub use automaton::{Automaton, build_automaton, deserialize_automaton, serialize_automaton};
pub use container::{MAGIC, VERSION, parse_container, write_container};
