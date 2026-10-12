pub mod automaton;
pub mod container;

pub use automaton::{Automaton, build_automaton, deserialize_automaton, serialize_automaton};
pub use container::{MAGIC, VERSION, parse_container, write_container};
