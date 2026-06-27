//! Pure operations over the canonical model. No I/O, no randomness, no clock.
//! Anything non-deterministic (new session ids, timestamps) is supplied by the
//! caller so these functions stay trivially testable.

use crate::error::{Error, Result};
use crate::model::{Message, Session};

/// Walk parent links from `uuid` up toward a root, returning the path in
/// root→`uuid` order.
///
/// Real Claude sessions are resumed and compacted, so a message's `parentUuid`
/// can reference a parent that lives in an earlier file or a compaction summary
/// and is therefore absent here. That is a normal *continuation boundary*, not
/// corruption — we stop there and treat that message as an effective root,
/// returning the longest lineage we can actually reconstruct. Cycles are still
/// an error; an unknown starting `uuid` is still `MessageNotFound`.
pub fn lineage<'a>(session: &'a Session, uuid: &str) -> Result<Vec<&'a Message>> {
    if session.message(uuid).is_none() {
        return Err(Error::MessageNotFound(uuid.to_string()));
    }
    let mut path: Vec<&Message> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cur = Some(uuid.to_string());
    while let Some(id) = cur {
        if !seen.insert(id.clone()) {
            return Err(Error::Cycle(id));
        }
        match session.message(&id) {
            Some(msg) => {
                path.push(msg);
                cur = msg.parent_uuid.clone();
            }
            // Parent referenced but not present: continuation boundary — stop.
            None => break,
        }
    }
    path.reverse();
    Ok(path)
}

/// File-order position of a message within the session's append log.
pub fn index_of(session: &Session, uuid: &str) -> Result<usize> {
    session
        .messages
        .iter()
        .position(|m| m.uuid == uuid)
        .ok_or_else(|| Error::MessageNotFound(uuid.to_string()))
}

/// The append-log prefix up to and including `at_uuid`, in file order.
///
/// Real Claude session files are append-logs whose `parentUuid` links are
/// riddled with references to messages never written to the file (compaction
/// boundaries, tool/meta records). Claude reconstructs a session by *file
/// order*, not by walking parents — so do we. This is the correct, robust basis
/// for branch/revert: "the conversation as it was up to this point."
pub fn prefix_until<'a>(session: &'a Session, at_uuid: &str) -> Result<Vec<&'a Message>> {
    let idx = index_of(session, at_uuid)?;
    Ok(session.messages[..=idx].iter().collect())
}

/// Build a NEW session (`new_id`) containing the append-log prefix up to and
/// including `at_uuid`. Headers and metadata are carried over. This is the
/// engine behind both `branch` and non-destructive `revert`.
///
/// The returned session's messages are clones; their embedded provider data in
/// `extra` (e.g. sessionId fields) is left untouched — rewriting that to match
/// `new_id` is the adapter's job at save time.
pub fn branch_at(session: &Session, at_uuid: &str, new_id: impl Into<String>) -> Result<Session> {
    let prefix = prefix_until(session, at_uuid)?;
    Ok(Session {
        id: new_id.into(),
        headers: session.headers.clone(),
        messages: prefix.into_iter().cloned().collect(),
        meta: session.meta.clone(),
    })
}

/// Destructive reset: keep the SAME session id but drop everything after
/// `at_uuid` (the `--hard` revert). Returns a new value; the caller decides
/// whether to overwrite the original.
pub fn truncate_at(session: &Session, at_uuid: &str) -> Result<Session> {
    let prefix = prefix_until(session, at_uuid)?;
    Ok(Session {
        id: session.id.clone(),
        headers: session.headers.clone(),
        messages: prefix.into_iter().cloned().collect(),
        meta: session.meta.clone(),
    })
}

/// Where two lineages diverge. Compares the root→head paths of both sessions
/// (using each session's first leaf as head when none is given).
#[derive(Debug, Clone, PartialEq)]
pub struct Divergence {
    /// uuid of the last shared message, if the two share any prefix.
    pub common_ancestor: Option<String>,
    /// Number of shared messages from the root.
    pub common_len: usize,
    /// Messages unique to `a` after the split.
    pub only_a: Vec<String>,
    /// Messages unique to `b` after the split.
    pub only_b: Vec<String>,
}

/// Compute divergence between the append-log prefix of `a_head` in `a` and
/// `b_head` in `b` (file order — see [`prefix_until`]).
pub fn diff(a: &Session, a_head: &str, b: &Session, b_head: &str) -> Result<Divergence> {
    let pa = prefix_until(a, a_head)?;
    let pb = prefix_until(b, b_head)?;
    let mut common_len = 0;
    while common_len < pa.len()
        && common_len < pb.len()
        && pa[common_len].uuid == pb[common_len].uuid
    {
        common_len += 1;
    }
    let common_ancestor = if common_len == 0 {
        None
    } else {
        Some(pa[common_len - 1].uuid.clone())
    };
    Ok(Divergence {
        common_ancestor,
        common_len,
        only_a: pa[common_len..].iter().map(|m| m.uuid.clone()).collect(),
        only_b: pb[common_len..].iter().map(|m| m.uuid.clone()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Role;

    /// Build a linear session a→b→c→d with the given id.
    fn linear(id: &str) -> Session {
        let mut s = Session::new(id);
        s.messages = vec![
            Message::new("a", None, Role::User, "hi"),
            Message::new("b", Some("a".into()), Role::Assistant, "hello"),
            Message::new("c", Some("b".into()), Role::User, "do X"),
            Message::new("d", Some("c".into()), Role::Assistant, "done X"),
        ];
        s
    }

    #[test]
    fn lineage_full_chain() {
        let s = linear("s1");
        let path: Vec<_> = lineage(&s, "d").unwrap().iter().map(|m| m.uuid.clone()).collect();
        assert_eq!(path, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn lineage_partial() {
        let s = linear("s1");
        let path: Vec<_> = lineage(&s, "b").unwrap().iter().map(|m| m.uuid.clone()).collect();
        assert_eq!(path, vec!["a", "b"]);
    }

    #[test]
    fn lineage_missing_message() {
        let s = linear("s1");
        assert!(matches!(lineage(&s, "zz"), Err(Error::MessageNotFound(_))));
    }

    #[test]
    fn lineage_detects_cycle() {
        let mut s = Session::new("s1");
        s.messages = vec![
            Message::new("a", Some("b".into()), Role::User, "x"),
            Message::new("b", Some("a".into()), Role::Assistant, "y"),
        ];
        assert!(matches!(lineage(&s, "a"), Err(Error::Cycle(_))));
    }

    #[test]
    fn lineage_tolerates_missing_parent() {
        // 'b' references parent 'missing' that isn't in the session (a real
        // continuation boundary). Lineage should stop at b, not error.
        let mut s = Session::new("s1");
        s.messages = vec![
            Message::new("b", Some("missing".into()), Role::User, "resumed"),
            Message::new("c", Some("b".into()), Role::Assistant, "ok"),
        ];
        let path: Vec<_> = lineage(&s, "c").unwrap().iter().map(|m| m.uuid.clone()).collect();
        assert_eq!(path, vec!["b", "c"]);
    }

    #[test]
    fn branch_at_new_id_and_slice() {
        let s = linear("s1");
        let b = branch_at(&s, "c", "s2").unwrap();
        assert_eq!(b.id, "s2");
        let ids: Vec<_> = b.messages.iter().map(|m| m.uuid.clone()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]); // d dropped
        // original untouched
        assert_eq!(s.messages.len(), 4);
    }

    #[test]
    fn branch_uses_file_order_through_phantom_parents() {
        // Mirrors real Claude data: messages in append order, but parent links
        // reference messages NOT in the file (compaction boundaries). Branching
        // must still return the full file-order prefix, not stop at a boundary.
        let mut s = Session::new("s1");
        s.messages = vec![
            Message::new("m0", None, Role::User, "start"),
            Message::new("m1", Some("PHANTOM".into()), Role::Assistant, "after compaction"),
            Message::new("m2", Some("m1".into()), Role::User, "good point"),
            Message::new("m3", Some("m2".into()), Role::Assistant, "later, bad"),
        ];
        let b = branch_at(&s, "m2", "s2").unwrap();
        let ids: Vec<_> = b.messages.iter().map(|m| m.uuid.clone()).collect();
        assert_eq!(ids, vec!["m0", "m1", "m2"]); // full prefix, m1's phantom parent ignored
    }

    #[test]
    fn truncate_keeps_id() {
        let s = linear("s1");
        let t = truncate_at(&s, "b").unwrap();
        assert_eq!(t.id, "s1");
        let ids: Vec<_> = t.messages.iter().map(|m| m.uuid.clone()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn diff_finds_divergence() {
        // base: a→b→c→d   ; fork branched at b then added e,f
        let base = linear("s1");
        let mut fork = Session::new("s2");
        fork.messages = vec![
            Message::new("a", None, Role::User, "hi"),
            Message::new("b", Some("a".into()), Role::Assistant, "hello"),
            Message::new("e", Some("b".into()), Role::User, "do Y"),
            Message::new("f", Some("e".into()), Role::Assistant, "done Y"),
        ];
        let d = diff(&base, "d", &fork, "f").unwrap();
        assert_eq!(d.common_ancestor.as_deref(), Some("b"));
        assert_eq!(d.common_len, 2);
        assert_eq!(d.only_a, vec!["c", "d"]);
        assert_eq!(d.only_b, vec!["e", "f"]);
    }

    #[test]
    fn children_and_leaves() {
        // a→b, a→c (a forked): leaves are b and c
        let mut s = Session::new("s1");
        s.messages = vec![
            Message::new("a", None, Role::User, "root"),
            Message::new("b", Some("a".into()), Role::Assistant, "b"),
            Message::new("c", Some("a".into()), Role::Assistant, "c"),
        ];
        assert_eq!(s.children("a").len(), 2);
        let mut leaves: Vec<_> = s.leaves().iter().map(|m| m.uuid.clone()).collect();
        leaves.sort();
        assert_eq!(leaves, vec!["b", "c"]);
    }
}
