//! Throwaway real-data smoke test:
//!   cargo run -p sily-adapter-claude --example realcheck -- <claude_home> <cwd> <session_id>
//! Loads a real session, reports stats, branches at the midpoint, saves to a
//! scratch claude_home, reloads, and checks fidelity. Never touches originals.

use sily_adapter_claude::ClaudeStore;
use sily_core::ops::branch_at;
use sily_core::store::SessionStore;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let claude_home = &args[1];
    let cwd = &args[2];
    let sid = &args[3];

    let store = ClaudeStore::new(claude_home, cwd.clone());
    let s = store.load(sid).expect("load real session");
    println!(
        "loaded {}: {} headers, {} messages",
        s.id,
        s.headers.len(),
        s.messages.len()
    );
    let roots = s.messages.iter().filter(|m| m.parent_uuid.is_none()).count();
    let broken = s
        .messages
        .iter()
        .filter(|m| {
            m.parent_uuid
                .as_ref()
                .map(|p| s.message(p).is_none())
                .unwrap_or(false)
        })
        .count();
    println!("roots={roots} broken_parent_links={broken}");

    if s.messages.len() < 2 {
        println!("too short to branch; done");
        return;
    }
    let mid = s.messages[s.messages.len() / 2].uuid.clone();

    // save the branch into a scratch home so originals are untouched
    let scratch = std::env::temp_dir().join("sily-realcheck");
    let _ = std::fs::remove_dir_all(&scratch);
    let scratch_store = ClaudeStore::new(&scratch, cwd.clone());
    let branched = branch_at(&s, &mid, "00000000-aaaa-bbbb-cccc-000000000001").unwrap();
    println!(
        "branched at midpoint {} -> {} messages",
        &mid[..8.min(mid.len())],
        branched.messages.len()
    );
    scratch_store.save(&branched).expect("save branch");
    let reloaded = scratch_store.load(&branched.id).expect("reload branch");

    // fidelity checks
    assert_eq!(branched.messages.len(), reloaded.messages.len());
    let mut stale = 0usize;
    let raw = std::fs::read_to_string(
        scratch_store.project_dir().join(format!("{}.jsonl", branched.id)),
    )
    .unwrap();
    for line in raw.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if let Some(x) = v.get("sessionId").and_then(|x| x.as_str()) {
            if x != branched.id {
                stale += 1;
            }
        }
    }
    println!("reloaded={} messages, stale_session_ids={stale}", reloaded.messages.len());
    println!("scratch file: {}", scratch_store.project_dir().join(format!("{}.jsonl", branched.id)).display());
    println!("OK");
}
