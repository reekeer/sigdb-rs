//! Byte-level Aho-Corasick automaton, its varint serialization and the two searches sigdb runs
//! on it: every occurrence (`contains`) and patterns anchored at the start (`prefix`).

use crate::error::{Error, Result};
use crate::utils::varint::{decode_varint, write_varint};

const NONE: u32 = u32::MAX;

/// The binary search of the Python package, kept step for step so that even a crafted file with
/// unsorted labels resolves the same edge.
#[inline]
fn search(labels: &[u8], b: u8) -> Option<usize> {
    let (mut lo, mut hi) = (0usize, labels.len());
    while lo < hi {
        let mid = (lo + hi) >> 1;
        let lb = labels[mid];
        if lb < b {
            lo = mid + 1;
        } else if lb > b {
            hi = mid;
        } else {
            return Some(mid);
        }
    }
    None
}

/// A compiled automaton in the flat layout it is stored in: per state a sorted run of edges and a
/// run of output pattern ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Automaton {
    pub children_start: Vec<u32>,
    pub children_count: Vec<u32>,
    pub fail: Vec<u32>,
    pub out_start: Vec<u32>,
    pub out_count: Vec<u32>,
    pub labels: Vec<u8>,
    pub next_state: Vec<u32>,
    pub outputs: Vec<u32>,
    root: Vec<u32>,
}

impl Automaton {
    fn with_root(mut self) -> Self {
        let mut root = vec![NONE; 256];
        if !self.children_start.is_empty() {
            let start = self.children_start[0] as usize;
            let count = self.children_count[0] as usize;
            let labels = &self.labels[start..start + count];
            for (b, slot) in root.iter_mut().enumerate() {
                if let Some(i) = search(labels, b as u8) {
                    *slot = self.next_state[start + i];
                }
            }
        }
        self.root = root;
        self
    }

    pub fn node_count(&self) -> usize {
        self.children_start.len()
    }

    /// Next state on byte `b`, or `None` without an edge.
    #[inline]
    pub fn transition(&self, state: u32, b: u8) -> Option<u32> {
        if state == 0 {
            let n = self.root[b as usize];
            return (n != NONE).then_some(n);
        }
        let s = state as usize;
        let start = self.children_start[s] as usize;
        let count = self.children_count[s] as usize;
        search(&self.labels[start..start + count], b).map(|i| self.next_state[start + i])
    }

    #[inline]
    pub fn outputs_of(&self, state: u32) -> &[u32] {
        let s = state as usize;
        let start = self.out_start[s] as usize;
        &self.outputs[start..start + self.out_count[s] as usize]
    }

    /// Every match as `(end, pattern_id)`, `end` being the byte offset just past it.
    pub fn find_contains(&self, data: &[u8]) -> Vec<(usize, u32)> {
        let mut found = Vec::new();
        self.for_each_contains(data, |end, pid| found.push((end, pid)));
        found
    }

    pub(crate) fn for_each_contains(&self, data: &[u8], mut f: impl FnMut(usize, u32)) {
        let mut state = 0u32;
        for (i, &b) in data.iter().enumerate() {
            loop {
                if let Some(n) = self.transition(state, b) {
                    state = n;
                    break;
                }
                if state == 0 {
                    break;
                }
                state = self.fail[state as usize];
            }
            if self.out_count[state as usize] != 0 {
                for &pid in self.outputs_of(state) {
                    f(i + 1, pid);
                }
            }
        }
    }

    /// Patterns that are a prefix of `data` as `(length, pattern_id)`; `lengths` holds the byte
    /// length of every pattern id.
    pub fn find_prefix(&self, data: &[u8], lengths: &[usize]) -> Vec<(usize, u32)> {
        let mut found = Vec::new();
        let mut state = 0u32;
        for (i, &b) in data.iter().enumerate() {
            let depth = i + 1;
            match self.transition(state, b) {
                Some(n) => state = n,
                None => break,
            }
            if self.out_count[state as usize] != 0 {
                for &pid in self.outputs_of(state) {
                    if lengths[pid as usize] == depth {
                        found.push((depth, pid));
                    }
                }
            }
        }
        found
    }
}

/// Builds the automaton for `(pattern bytes, pattern id)` pairs, numbering states in insertion
/// order exactly as the Python package does.
pub fn build_automaton<P: AsRef<[u8]>>(patterns: &[(P, u32)]) -> Automaton {
    let mut trans: Vec<Vec<(u8, u32)>> = vec![Vec::new()];
    let mut out: Vec<Vec<u32>> = vec![Vec::new()];

    for (bytes, pid) in patterns {
        let mut state = 0usize;
        for &b in bytes.as_ref() {
            let edges = &trans[state];
            let next = match edges.binary_search_by_key(&b, |&(l, _)| l) {
                Ok(i) => edges[i].1 as usize,
                Err(i) => {
                    let n = trans.len();
                    trans[state].insert(i, (b, n as u32));
                    trans.push(Vec::new());
                    out.push(Vec::new());
                    n
                }
            };
            state = next;
        }
        out[state].push(*pid);
    }

    for o in out.iter_mut() {
        if o.len() > 1 {
            o.sort_unstable();
            o.dedup();
        }
    }

    let goto = |trans: &Vec<Vec<(u8, u32)>>, s: usize, b: u8| -> Option<u32> {
        let edges = &trans[s];
        edges.binary_search_by_key(&b, |&(l, _)| l).ok().map(|i| edges[i].1)
    };

    let n = trans.len();
    let mut fail = vec![0u32; n];
    let mut queue: std::collections::VecDeque<usize> = trans[0].iter().map(|&(_, u)| u as usize).collect();
    while let Some(v) = queue.pop_front() {
        for k in 0..trans[v].len() {
            let (b, u) = trans[v][k];
            let u = u as usize;
            queue.push_back(u);
            let mut f = fail[v] as usize;
            while f != 0 && goto(&trans, f, b).is_none() {
                f = fail[f] as usize;
            }
            fail[u] = goto(&trans, f, b).unwrap_or(0);
            let fu = fail[u] as usize;
            if !out[fu].is_empty() {
                let mut merged = out[u].clone();
                merged.extend_from_slice(&out[fu]);
                merged.sort_unstable();
                merged.dedup();
                out[u] = merged;
            }
        }
    }

    let mut children_start = Vec::with_capacity(n);
    let mut children_count = Vec::with_capacity(n);
    let mut out_start = Vec::with_capacity(n);
    let mut out_count = Vec::with_capacity(n);
    let mut labels = Vec::with_capacity(n.saturating_sub(1));
    let mut next_state = Vec::with_capacity(n.saturating_sub(1));
    let mut outputs = Vec::new();
    for i in 0..n {
        children_start.push(labels.len() as u32);
        children_count.push(trans[i].len() as u32);
        for &(b, nxt) in &trans[i] {
            labels.push(b);
            next_state.push(nxt);
        }
        out_start.push(outputs.len() as u32);
        out_count.push(out[i].len() as u32);
        outputs.extend_from_slice(&out[i]);
    }

    Automaton {
        children_start,
        children_count,
        fail,
        out_start,
        out_count,
        labels,
        next_state,
        outputs,
        root: Vec::new(),
    }
    .with_root()
}

pub fn serialize_automaton(a: &Automaton) -> Vec<u8> {
    let n = a.node_count();
    let mut out = Vec::with_capacity(n * 5 + a.labels.len() * 3 + a.outputs.len() * 2 + 16);
    write_varint(&mut out, n as u64);
    write_varint(&mut out, a.labels.len() as u64);
    write_varint(&mut out, a.outputs.len() as u64);
    for i in 0..n {
        write_varint(&mut out, a.children_start[i] as u64);
        write_varint(&mut out, a.children_count[i] as u64);
        write_varint(&mut out, a.fail[i] as u64);
        write_varint(&mut out, a.out_start[i] as u64);
        write_varint(&mut out, a.out_count[i] as u64);
    }
    out.extend_from_slice(&a.labels);
    for &nxt in &a.next_state {
        write_varint(&mut out, nxt as u64);
    }
    for &pid in &a.outputs {
        write_varint(&mut out, pid as u64);
    }
    out
}

pub fn deserialize_automaton(data: &[u8], pattern_count: usize) -> Result<Automaton> {
    let mut pos = 0usize;
    let read = |pos: &mut usize| -> Result<u64> {
        let r = decode_varint(data, *pos, 10)?;
        *pos = r.offset;
        Ok(r.value)
    };

    let node_count = read(&mut pos)?;
    let edge_count = read(&mut pos)?;
    let output_total = read(&mut pos)?;
    let need = (node_count as u128) * 5 + (edge_count as u128) * 2 + output_total as u128;
    if node_count < 1 || need > data.len() as u128 {
        return Err(Error::format("automaton counts exceed block size"));
    }
    let (n, edges, outs) = (node_count as usize, edge_count as usize, output_total as usize);

    let mut raw = vec![[0u64; 5]; n];
    for row in raw.iter_mut() {
        for v in row.iter_mut() {
            *v = read(&mut pos)?;
        }
    }

    if pos + edges > data.len() {
        return Err(Error::format("truncated automaton labels"));
    }
    let labels = data[pos..pos + edges].to_vec();
    pos += edges;
    let mut next_raw = Vec::with_capacity(edges);
    for _ in 0..edges {
        next_raw.push(read(&mut pos)?);
    }
    let mut out_raw = Vec::with_capacity(outs);
    for _ in 0..outs {
        out_raw.push(read(&mut pos)?);
    }

    if pos != data.len() {
        return Err(Error::format("automaton block has trailing bytes"));
    }

    for row in &raw {
        let [cs, cc, fl, os, oc] = *row;
        if cs as u128 + cc as u128 > edge_count as u128 {
            return Err(Error::format("automaton edge range out of bounds"));
        }
        if os as u128 + oc as u128 > output_total as u128 {
            return Err(Error::format("automaton output range out of bounds"));
        }
        if fl >= node_count {
            return Err(Error::format("automaton fail link out of bounds"));
        }
    }
    if next_raw.iter().any(|&s| s >= node_count || s == 0) {
        return Err(Error::format("automaton transition out of bounds"));
    }
    if out_raw.iter().any(|&p| p >= pattern_count as u64) {
        return Err(Error::format("automaton output references unknown pattern"));
    }

    let col = |k: usize| raw.iter().map(|r| r[k] as u32).collect::<Vec<u32>>();
    let automaton = Automaton {
        children_start: col(0),
        children_count: col(1),
        fail: col(2),
        out_start: col(3),
        out_count: col(4),
        labels,
        next_state: next_raw.into_iter().map(|s| s as u32).collect(),
        outputs: out_raw.into_iter().map(|p| p as u32).collect(),
        root: Vec::new(),
    }
    .with_root();
    check_fail_links(&automaton)?;
    Ok(automaton)
}

/// Rejects fail links that do not lead to a shallower state, which would make a search loop
/// forever (the Python package does not check this; sigdb-ts does, with the same message).
fn check_fail_links(a: &Automaton) -> Result<()> {
    let n = a.node_count();
    let mut depth = vec![-1i64; n];
    depth[0] = 0;
    let mut queue = vec![0usize];
    let mut qi = 0;
    while qi < queue.len() {
        let v = queue[qi];
        qi += 1;
        let start = a.children_start[v] as usize;
        for e in start..start + a.children_count[v] as usize {
            let u = a.next_state[e] as usize;
            if depth[u] == -1 {
                depth[u] = depth[v] + 1;
                queue.push(u);
            }
        }
    }
    for &v in &queue {
        if v != 0 && depth[a.fail[v] as usize] >= depth[v] {
            return Err(Error::format("automaton fail links do not terminate"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_search() {
        let pats: Vec<(&[u8], u32)> = vec![(b"he", 0), (b"she", 1), (b"his", 2), (b"hers", 3)];
        let a = build_automaton(&pats);
        let ser = serialize_automaton(&a);
        let back = deserialize_automaton(&ser, 4).unwrap();
        assert_eq!(a, back);
        let found = a.find_contains(b"ushers");
        assert_eq!(found, vec![(4, 0), (4, 1), (6, 3)]);
        let lengths = vec![2, 3, 3, 4];
        assert_eq!(a.find_prefix(b"hersx", &lengths), vec![(2, 0), (4, 3)]);
    }

    #[test]
    fn errors() {
        assert_eq!(
            deserialize_automaton(&[0, 0, 0], 0).unwrap_err().message(),
            "automaton counts exceed block size"
        );
        let a = build_automaton(&[(b"ab".as_slice(), 0)]);
        let mut ser = serialize_automaton(&a);
        ser.push(0);
        assert_eq!(
            deserialize_automaton(&ser, 1).unwrap_err().message(),
            "automaton block has trailing bytes"
        );
        let ser = serialize_automaton(&a);
        assert_eq!(
            deserialize_automaton(&ser, 0).unwrap_err().message(),
            "automaton output references unknown pattern"
        );
    }
}
