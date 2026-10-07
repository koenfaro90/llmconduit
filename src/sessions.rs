//! Session tree and prefix lineage.
//!
//! Every persisted request belongs to exactly one **session node**. Nodes nest
//! arbitrarily (session, sub-session, sub-sub-session) and are either
//! *declared* — the harness put the id on the wire (see `crate::harness`) —
//! or *inferred* — a **conversation node** the gateway opened for a client
//! that declared no session id. Inference is content-based only: an
//! undeclared request is matched against that client's known conversation
//! heads by shared conversation messages, and a request that matches nothing
//! opens its own conversation node. There is no per-client bucket and no
//! chaining after the previous head — both silently merged unrelated
//! conversations into one node, which was worse than no attribution.
//!
//! Sub-sessions are never inferred: a request inside a declared session that
//! matches no chain simply starts a new chain on that session's node.
//!
//! Each node owns one **chain**: the sequence of requests that each extend the
//! previous one's item list. Linking a request means finding the chain whose
//! item hashes are the longest prefix of the new request's, recording the
//! predecessor and the **divergence** (where and how the new request differs),
//! and advancing the chain. A divergence inside the shared prefix is what
//! breaks an upstream KV/prefix cache, so it is stored as a flag per request.
//!
//! The linker is an in-memory index guarded by a mutex; it returns the rows to
//! persist and never touches the store itself. It is bounded (LRU over nodes)
//! and can be seeded from durable rows after a restart.

use crate::content_store::{ItemSection, SplitItem};
use crate::harness::{HarnessIdentity, SubSessionPolicy};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

/// Upper bound on session nodes kept in memory; oldest-touched are evicted.
pub const MAX_NODES: usize = 20_000;
/// Upper bound on item hashes retained per chain head.
pub const MAX_HEAD_ITEMS: usize = 8_192;
/// Upper bound on conversation nodes examined when matching an undeclared
/// request, and on the per-client candidate list retained for that matching.
const MAX_CANDIDATE_CHAINS: usize = 256;

pub const KIND_DECLARED: &str = "declared";
pub const KIND_INFERRED: &str = "inferred";

/// The per-client key conversation nodes are matched under: the client label
/// (key hash / configured id), else the virtual key id, else the empty
/// string. Shared with the warm-up path so both agree on the bucket.
pub fn conversation_client_key(client_label: Option<&str>, virtual_key_id: Option<&str>) -> String {
    client_label
        .or(virtual_key_id)
        .unwrap_or_default()
        .to_string()
}

/// Whether two prefix-less item lists are the same conversation with some
/// items changed (a rewritten system prompt, an edited tool) rather than two
/// unrelated requests that happen to share a message: the messages in common
/// (see [`overlap`]) must cover at least half of the conversation messages of
/// the larger side. A title call that repeats the user's one message is not
/// a rewrite of a 34-item chain; a re-sent conversation with a new system
/// prompt still is.
fn overlap_is_a_match(common: usize, head: &[ItemFingerprint], input: &[ItemFingerprint]) -> bool {
    common >= 1 && common * 2 >= conversation_messages(head).max(conversation_messages(input))
}

/// The number of user/assistant/tool turns (the population [`overlap`] counts).
fn conversation_messages(items: &[ItemFingerprint]) -> usize {
    items
        .iter()
        .filter(|item| {
            item.section == ItemSection::Message
                && !matches!(item.kind.as_deref(), Some("system") | Some("developer"))
        })
        .count()
}

/// The part of a split item that lineage needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemFingerprint {
    pub hash: String,
    pub section: ItemSection,
    pub kind: Option<String>,
}

impl From<&SplitItem> for ItemFingerprint {
    fn from(item: &SplitItem) -> Self {
        Self {
            hash: item.identity.clone(),
            section: item.section,
            kind: item.kind.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DivergenceKind {
    /// The predecessor's items are a full prefix; only new items were added.
    Append,
    /// The instructions/system block (or a leading system message) changed.
    InstructionsChanged,
    /// The tool list changed.
    ToolsChanged,
    /// An earlier message inside the shared history changed or was removed.
    HistoryRewritten,
    /// Nothing in common with any known chain: the start of a conversation.
    NewChain,
}

impl DivergenceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::InstructionsChanged => "instructions_changed",
            Self::ToolsChanged => "tools_changed",
            Self::HistoryRewritten => "history_rewritten",
            Self::NewChain => "new_chain",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lineage {
    pub kind: DivergenceKind,
    /// Items shared with the predecessor before the first difference.
    pub shared_prefix: usize,
    /// Index of the first differing item, absent for a pure append.
    pub index: Option<usize>,
    /// True when the divergence falls inside the predecessor's items, which
    /// is what invalidates an upstream prefix cache.
    pub cache_bust: bool,
}

/// Length of the common prefix of two item sequences.
fn shared_prefix(predecessor: &[ItemFingerprint], current: &[ItemFingerprint]) -> usize {
    predecessor
        .iter()
        .zip(current.iter())
        .take_while(|(a, b)| a.hash == b.hash)
        .count()
}

/// How many of the predecessor's conversation messages reappear anywhere in
/// `current`. Only user/assistant/tool turns count: instructions and tool
/// definitions are shared by every conversation a harness runs (a Claude
/// Code sub-agent has the same tools as the main thread), so they cannot
/// tell "the system prompt changed, same conversation" (cache bust) apart
/// from "a different conversation" (new chain). Shared history can.
fn overlap(predecessor: &[ItemFingerprint], current: &[ItemFingerprint]) -> usize {
    let present: HashSet<&str> = current.iter().map(|item| item.hash.as_str()).collect();
    predecessor
        .iter()
        .filter(|item| {
            item.section == ItemSection::Message
                && !matches!(item.kind.as_deref(), Some("system") | Some("developer"))
        })
        .filter(|item| present.contains(item.hash.as_str()))
        .count()
}

/// Compare a request's items with its chain predecessor's.
pub fn classify(predecessor: &[ItemFingerprint], current: &[ItemFingerprint]) -> Lineage {
    let shared = shared_prefix(predecessor, current);
    if shared == predecessor.len() {
        return Lineage {
            kind: DivergenceKind::Append,
            shared_prefix: shared,
            index: None,
            cache_bust: false,
        };
    }
    // The request is a strict prefix of its predecessor: the conversation was
    // rewound (or a new one started from the same prompt, for a harness with
    // no session id). Everything it sends is still in the KV cache.
    if shared == current.len() {
        return Lineage {
            kind: DivergenceKind::HistoryRewritten,
            shared_prefix: shared,
            index: Some(shared),
            cache_bust: false,
        };
    }
    if shared == 0 && overlap(predecessor, current) == 0 {
        return Lineage {
            kind: DivergenceKind::NewChain,
            shared_prefix: 0,
            index: Some(0),
            cache_bust: false,
        };
    }
    // Look at BOTH sides of the divergence point: an item inserted into the
    // current request (a tool appended to the list) shows up on the current
    // side while the predecessor's item there is the first shifted message.
    let kind = [predecessor.get(shared), current.get(shared)]
        .into_iter()
        .flatten()
        .map(|item| match item.section {
            ItemSection::Instructions => DivergenceKind::InstructionsChanged,
            ItemSection::Tool => DivergenceKind::ToolsChanged,
            ItemSection::Message => match item.kind.as_deref() {
                Some("system") | Some("developer") => DivergenceKind::InstructionsChanged,
                _ => DivergenceKind::HistoryRewritten,
            },
        })
        // Instructions and tools outrank a history rewrite when either side says so.
        .min_by_key(|kind| match kind {
            DivergenceKind::InstructionsChanged => 0,
            DivergenceKind::ToolsChanged => 1,
            _ => 2,
        })
        .unwrap_or(DivergenceKind::HistoryRewritten);
    Lineage {
        kind,
        shared_prefix: shared,
        index: Some(shared),
        cache_bust: true,
    }
}

/// Durable shape of a session node (table `sessions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SessionRow {
    pub id: String,
    #[serde(default)]
    pub display_number: Option<i64>,
    pub parent_id: Option<String>,
    pub kind: String,
    pub harness: String,
    pub harness_version: Option<String>,
    pub external_id: Option<String>,
    pub session_kind: Option<String>,
    pub client_label: Option<String>,
    pub virtual_key_id: Option<String>,
    /// Owner of the key that opened the node; `None` for open-mode requests,
    /// legacy rows, and keys without an owner (migration 0012).
    #[serde(default)]
    pub user_id: Option<String>,
    pub depth: i64,
    pub root_request_id: Option<String>,
    pub spawned_by_request_id: Option<String>,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    pub request_count: i64,
}

/// What linking produced for one request. The session node always exists: a
/// declared request lands on its declared node, an undeclared request either
/// extends the conversation it content-matches or opens a fresh conversation
/// node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLink {
    /// The node whose chain the request extends or starts.
    pub session_id: String,
    pub chain_parent_request_id: Option<String>,
    pub lineage: Lineage,
    pub item_count: usize,
    /// Nodes created or touched; persist them (upsert) in order.
    pub upserts: Vec<SessionRow>,
}
pub struct LinkInput<'a> {
    pub api_call_id: &'a str,
    pub identity: &'a HarnessIdentity,
    pub client_label: Option<&'a str>,
    pub virtual_key_id: Option<&'a str>,
    /// Owner of the authenticating virtual key, when known.
    pub user_id: Option<&'a str>,
    pub items: &'a [ItemFingerprint],
    pub now_ms: i64,
}

/// The latest request of a node's chain and its items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainHead {
    pub request_id: String,
    pub items: Vec<ItemFingerprint>,
}

#[derive(Debug)]
struct Node {
    row: SessionRow,
    head: Option<ChainHead>,
    children: Vec<String>,
}

#[derive(Debug, Default)]
struct Index {
    display_numbers: crate::control_plane_store::DisplayNumbers,
    nodes: HashMap<String, Node>,
    declared: HashMap<(String, String), String>,
    /// Conversation nodes per (harness, client key), most recent first.
    /// Entries may reference evicted nodes; matching skips missing ones.
    conversations: HashMap<(String, String), Vec<String>>,
    /// Touch order for eviction (front = oldest). May hold stale ids.
    order: VecDeque<String>,
}

/// In-memory session index. Cheap to clone the handle (`Arc` it).
#[derive(Debug, Default)]
pub struct SessionLinker {
    index: std::sync::Arc<Mutex<Index>>,
}

impl SessionLinker {
    pub fn new() -> Self {
        Self::with_display_numbers(crate::control_plane_store::DisplayNumbers::default())
    }

    pub fn with_display_numbers(
        display_numbers: crate::control_plane_store::DisplayNumbers,
    ) -> Self {
        Self {
            index: std::sync::Arc::new(Mutex::new(Index {
                display_numbers,
                ..Index::default()
            })),
        }
    }

    /// Drop remembered lineage after durable history is cleared, so the next
    /// request starts a fresh chain instead of referencing a deleted parent.
    pub fn clear(&self) {
        let mut index = self
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let display_numbers = index.display_numbers.clone();
        *index = Index {
            display_numbers,
            ..Index::default()
        };
    }

    /// Whether a declared session is already known in memory. The caller uses
    /// this to decide whether to warm the node from durable rows first.
    pub fn knows_declared(&self, harness: &str, external_id: &str) -> bool {
        let index = self
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        index
            .declared
            .contains_key(&(harness.to_string(), external_id.to_string()))
    }

    /// Whether any conversation node for this client is already in memory.
    /// The caller uses this to decide whether to warm the client's recent
    /// conversations from durable rows first.
    pub fn knows_conversations(&self, harness: &str, client_label: Option<&str>) -> bool {
        let index = self
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        index.conversations.contains_key(&(
            harness.to_string(),
            client_label.unwrap_or_default().to_string(),
        ))
    }

    /// Seed nodes (and optionally their chain heads) from durable rows, for
    /// example after a restart. Existing in-memory nodes are left untouched.
    pub fn seed(&self, rows: Vec<(SessionRow, Option<ChainHead>)>) {
        let mut index = self
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (row, head) in rows {
            if index.nodes.contains_key(&row.id) {
                continue;
            }
            if let Some(number) = row.display_number {
                index.display_numbers.observe_session(number);
            }
            index.insert_node(row, head);
        }
    }

    pub fn node_count(&self) -> usize {
        self.index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .nodes
            .len()
    }

    /// Link one request. Pure in-memory work; the returned `upserts` are the
    /// rows the caller must persist.
    pub fn link(&self, input: LinkInput<'_>) -> SessionLink {
        let mut index = self
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let identity = input.identity;
        let mut touched: Vec<String> = Vec::new();

        // 1. The node the request belongs to.
        let (node_id, lineage, chain_parent) = match identity.session_id.as_deref() {
            // A declared session id is exact attribution. Sub-sessions nest
            // only when declared (pi-agent/codex/claude-code style); a request
            // that matches no chain simply starts a new chain on its node,
            // chained after the node's previous head.
            Some(session_id) => {
                let anchor = match (identity.sub_sessions, identity.parent_session_id.as_deref()) {
                    // A declared session naming a parent (pi-agent sub-agent
                    // process): nest it under that parent, creating a
                    // placeholder when the parent has not been seen yet.
                    (SubSessionPolicy::Declared, Some(parent_id))
                        if identity.sub_session_id.is_none() =>
                    {
                        Some(index.declared_node(identity, parent_id, None, &input, &mut touched))
                    }
                    _ => None,
                };
                let session = index.declared_node(
                    identity,
                    session_id,
                    anchor.as_deref(),
                    &input,
                    &mut touched,
                );
                let node_id = match (identity.sub_sessions, identity.sub_session_id.as_deref()) {
                    (SubSessionPolicy::Declared, Some(sub_id)) => {
                        let parent = match identity.parent_session_id.as_deref() {
                            Some(parent_id) => index.declared_node(
                                identity,
                                parent_id,
                                Some(&session),
                                &input,
                                &mut touched,
                            ),
                            None => session.clone(),
                        };
                        index.declared_node(identity, sub_id, Some(&parent), &input, &mut touched)
                    }
                    _ => session,
                };
                let (lineage, chain_parent) = index
                    .nodes
                    .get(&node_id)
                    .and_then(|node| node.head.as_ref())
                    .map(|head| {
                        (
                            classify(&head.items, input.items),
                            Some(head.request_id.clone()),
                        )
                    })
                    .unwrap_or_else(|| (new_chain(), None));
                (node_id, lineage, chain_parent)
            }
            // No declared session id: content-match against this client's
            // known conversations. Instructions and tools are shared by every
            // conversation a harness runs, so only shared conversation
            // messages count as a match — a request that shares just the
            // system prompt (the production failure: thousands of unrelated
            // conversations chained onto one node) matches nothing and opens
            // its own conversation node.
            None => {
                let client_key = conversation_client_key(input.client_label, input.virtual_key_id);
                let candidates = index
                    .conversations
                    .get(&(identity.harness.clone(), client_key.clone()))
                    .cloned()
                    .unwrap_or_default();
                let mut best: Option<(String, (usize, usize))> = None;
                for candidate in candidates.iter().take(MAX_CANDIDATE_CHAINS) {
                    let Some(head) = index
                        .nodes
                        .get(candidate)
                        .and_then(|node| node.head.as_ref())
                    else {
                        continue;
                    };
                    let prefix = shared_prefix(&head.items, input.items);
                    let common = overlap(&head.items, input.items);
                    if common == 0 {
                        continue;
                    }
                    // Score = (shared prefix, set overlap): a chain that
                    // shares a prefix beats one that only shares items; among
                    // prefix-less matches the one with more conversation
                    // messages in common wins (a changed system prompt keeps
                    // the rest of the conversation). An overlap-only match
                    // must be substantial (see [`overlap_is_a_match`]): a
                    // side call that shares one user message with the main
                    // conversation is not its rewrite.
                    if prefix == 0 && !overlap_is_a_match(common, &head.items, input.items) {
                        continue;
                    }
                    let score = (prefix, common);
                    if best.as_ref().is_none_or(|(_, current)| score > *current) {
                        best = Some((candidate.clone(), score));
                    }
                }
                match best {
                    Some((node_id, _)) => {
                        let head = index
                            .nodes
                            .get(&node_id)
                            .and_then(|node| node.head.as_ref())
                            .expect("best candidate has a head")
                            .clone();
                        index.promote_conversation(&identity.harness, &client_key, &node_id);
                        (
                            node_id,
                            classify(&head.items, input.items),
                            Some(head.request_id),
                        )
                    }
                    None => {
                        let node_id = index.conversation_node(identity, &input, &mut touched);
                        (node_id, new_chain(), None)
                    }
                }
            }
        };

        // 2. Advance the chosen chain and account the request on its node.
        if let Some(node) = index.nodes.get_mut(&node_id) {
            let mut items = input.items.to_vec();
            items.truncate(MAX_HEAD_ITEMS);
            node.head = Some(ChainHead {
                request_id: input.api_call_id.to_string(),
                items,
            });
            node.row.request_count += 1;
            node.row.last_seen_ms = input.now_ms;
            if node.row.root_request_id.is_none() {
                node.row.root_request_id = Some(input.api_call_id.to_string());
            }
            if node.row.harness_version.is_none() {
                node.row.harness_version = identity.version.clone();
            }
        }
        index.touch(&node_id);
        if !touched.contains(&node_id) {
            touched.push(node_id.clone());
        }
        index.evict();

        let upserts = touched
            .iter()
            .filter_map(|id| index.nodes.get(id).map(|node| node.row.clone()))
            .collect();
        SessionLink {
            session_id: node_id,
            chain_parent_request_id: chain_parent,
            lineage,
            item_count: input.items.len(),
            upserts,
        }
    }
}

fn new_chain() -> Lineage {
    Lineage {
        kind: DivergenceKind::NewChain,
        shared_prefix: 0,
        index: None,
        cache_bust: false,
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Index {
    fn insert_node(&mut self, row: SessionRow, head: Option<ChainHead>) {
        if let Some(external_id) = &row.external_id {
            self.declared
                .insert((row.harness.clone(), external_id.clone()), row.id.clone());
        } else if row.kind == KIND_INFERRED && row.parent_id.is_none() {
            let key = (
                row.harness.clone(),
                conversation_client_key(row.client_label.as_deref(), row.virtual_key_id.as_deref()),
            );
            let list = self.conversations.entry(key).or_default();
            list.retain(|id| id != &row.id);
            list.insert(0, row.id.clone());
            list.truncate(MAX_CANDIDATE_CHAINS);
        }
        if let Some(parent_id) = &row.parent_id
            && let Some(parent) = self.nodes.get_mut(parent_id)
        {
            parent.children.push(row.id.clone());
        }
        self.order.push_back(row.id.clone());
        self.nodes.insert(
            row.id.clone(),
            Node {
                row,
                head,
                children: Vec::new(),
            },
        );
    }

    fn touch(&mut self, id: &str) {
        self.order.push_back(id.to_string());
    }

    fn evict(&mut self) {
        while self.nodes.len() > MAX_NODES {
            let Some(candidate) = self.order.pop_front() else {
                break;
            };
            // A later touch keeps the node alive; only evict if this was its
            // last (front-most) position.
            if self.order.iter().any(|id| id == &candidate) {
                continue;
            }
            if let Some(node) = self.nodes.remove(&candidate) {
                if let Some(external_id) = &node.row.external_id {
                    self.declared
                        .remove(&(node.row.harness.clone(), external_id.clone()));
                }
                // Conversation lists keep evicted ids; matching skips missing
                // nodes and the cap bounds the stale tail.
                if let Some(parent_id) = &node.row.parent_id
                    && let Some(parent) = self.nodes.get_mut(parent_id)
                {
                    parent.children.retain(|child| child != &candidate);
                }
            }
        }
        // Keep the touch log from growing without bound.
        if self.order.len() > MAX_NODES.saturating_mul(4) {
            let mut seen = HashSet::new();
            let mut compact = VecDeque::new();
            for id in self.order.iter().rev() {
                if seen.insert(id.clone()) {
                    compact.push_front(id.clone());
                }
            }
            self.order = compact;
        }
    }

    /// Find or create the declared node for `external_id`.
    fn declared_node(
        &mut self,
        identity: &HarnessIdentity,
        external_id: &str,
        parent_id: Option<&str>,
        input: &LinkInput<'_>,
        touched: &mut Vec<String>,
    ) -> String {
        let key = (identity.harness.clone(), external_id.to_string());
        if let Some(id) = self.declared.get(&key).cloned() {
            // A node first seen as a bare session may learn its parent later.
            if let Some(parent_id) = parent_id
                && parent_id != id
                && self
                    .nodes
                    .get(&id)
                    .is_some_and(|node| node.row.parent_id.is_none())
            {
                let depth = self
                    .nodes
                    .get(parent_id)
                    .map(|parent| parent.row.depth + 1)
                    .unwrap_or(1);
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.row.parent_id = Some(parent_id.to_string());
                    node.row.depth = depth;
                }
                if let Some(parent) = self.nodes.get_mut(parent_id) {
                    parent.children.push(id.clone());
                }
                if !touched.contains(&id) {
                    touched.push(id.clone());
                }
            }
            return id;
        }
        let depth = parent_id
            .and_then(|parent| self.nodes.get(parent))
            .map(|parent| parent.row.depth + 1)
            .unwrap_or(0);
        let row = SessionRow {
            id: new_id(),
            display_number: Some(self.display_numbers.next_session()),
            parent_id: parent_id.map(str::to_string),
            kind: KIND_DECLARED.to_string(),
            harness: identity.harness.clone(),
            harness_version: identity.version.clone(),
            external_id: Some(external_id.to_string()),
            session_kind: identity.session_kind.clone(),
            client_label: input.client_label.map(str::to_string),
            virtual_key_id: input.virtual_key_id.map(str::to_string),
            user_id: input.user_id.map(str::to_string),
            depth,
            root_request_id: None,
            spawned_by_request_id: None,
            first_seen_ms: input.now_ms,
            last_seen_ms: input.now_ms,
            request_count: 0,
        };
        let id = row.id.clone();
        touched.push(id.clone());
        self.insert_node(row, None);
        id
    }

    /// Open a fresh conversation node for an undeclared request that matched
    /// no known conversation. Registered under the client's candidate list so
    /// later turns of the same conversation can match it.
    fn conversation_node(
        &mut self,
        identity: &HarnessIdentity,
        input: &LinkInput<'_>,
        touched: &mut Vec<String>,
    ) -> String {
        let row = SessionRow {
            id: new_id(),
            display_number: Some(self.display_numbers.next_session()),
            parent_id: None,
            kind: KIND_INFERRED.to_string(),
            harness: identity.harness.clone(),
            harness_version: identity.version.clone(),
            external_id: None,
            session_kind: identity.session_kind.clone(),
            client_label: input.client_label.map(str::to_string),
            virtual_key_id: input.virtual_key_id.map(str::to_string),
            user_id: input.user_id.map(str::to_string),
            depth: 0,
            root_request_id: None,
            spawned_by_request_id: None,
            first_seen_ms: input.now_ms,
            last_seen_ms: input.now_ms,
            request_count: 0,
        };
        let id = row.id.clone();
        touched.push(id.clone());
        self.insert_node(row, None);
        id
    }

    /// Move a matched conversation node to the front of the client's
    /// candidate list (most recent first).
    fn promote_conversation(&mut self, harness: &str, client_key: &str, node_id: &str) {
        if let Some(list) = self
            .conversations
            .get_mut(&(harness.to_string(), client_key.to_string()))
        {
            list.retain(|id| id != node_id);
            list.insert(0, node_id.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(hash: &str, section: ItemSection, kind: Option<&str>) -> ItemFingerprint {
        ItemFingerprint {
            hash: hash.to_string(),
            section,
            kind: kind.map(str::to_string),
        }
    }

    fn msg(hash: &str) -> ItemFingerprint {
        fp(hash, ItemSection::Message, Some("user"))
    }

    fn convo(hashes: &[&str]) -> Vec<ItemFingerprint> {
        let mut items = vec![
            fp("sys", ItemSection::Instructions, None),
            fp("tool-a", ItemSection::Tool, Some("a")),
        ];
        items.extend(hashes.iter().map(|hash| msg(hash)));
        items
    }

    fn identity(harness: &str, session: Option<&str>) -> HarnessIdentity {
        HarnessIdentity {
            harness: harness.to_string(),
            version: Some("1".to_string()),
            session_id: session.map(str::to_string),
            sub_session_id: None,
            parent_session_id: None,
            session_kind: None,
            sub_sessions: SubSessionPolicy::Declared,
        }
    }

    fn link<'a>(
        linker: &SessionLinker,
        id: &'a str,
        identity: &'a HarnessIdentity,
        items: &'a [ItemFingerprint],
        now: i64,
    ) -> SessionLink {
        link_with_client(linker, id, identity, "key-abc", items, now)
    }

    fn link_with_client<'a>(
        linker: &SessionLinker,
        id: &'a str,
        identity: &'a HarnessIdentity,
        client: &str,
        items: &'a [ItemFingerprint],
        now: i64,
    ) -> SessionLink {
        linker.link(LinkInput {
            api_call_id: id,
            identity,
            client_label: Some(client),
            virtual_key_id: None,
            user_id: None,
            items,
            now_ms: now,
        })
    }

    #[test]
    fn classify_covers_every_divergence_kind() {
        let base = convo(&["u1", "a1"]);
        let appended = convo(&["u1", "a1", "u2"]);
        let lineage = classify(&base, &appended);
        assert_eq!(lineage.kind, DivergenceKind::Append);
        assert_eq!(lineage.shared_prefix, 4);
        assert_eq!(lineage.index, None);
        assert!(!lineage.cache_bust);

        let mut new_sys = appended.clone();
        new_sys[0].hash = "sys2".into();
        let lineage = classify(&base, &new_sys);
        assert_eq!(lineage.kind, DivergenceKind::InstructionsChanged);
        assert_eq!(lineage.index, Some(0));
        assert!(lineage.cache_bust);

        let mut new_tools = appended.clone();
        new_tools[1].hash = "tool-b".into();
        let lineage = classify(&base, &new_tools);
        assert_eq!(lineage.kind, DivergenceKind::ToolsChanged);
        assert_eq!(lineage.index, Some(1));

        // A tool APPENDED to the list shifts every message by one: the
        // predecessor's item at the divergence is a message, the current
        // side's is the new tool. That is a tools change (smoke test 2026-09-12).
        let mut added_tool = appended.clone();
        added_tool.insert(2, fp("tool-b", ItemSection::Tool, Some("b")));
        let lineage = classify(&base, &added_tool);
        assert_eq!(lineage.kind, DivergenceKind::ToolsChanged);
        assert_eq!(lineage.index, Some(2));
        assert_eq!(lineage.shared_prefix, 2);
        assert!(lineage.cache_bust);
        // A tool REMOVED from the list: the predecessor side holds the tool.
        let mut removed_tool = base.clone();
        removed_tool.remove(1);
        assert_eq!(
            classify(&base, &removed_tool).kind,
            DivergenceKind::ToolsChanged
        );

        let mut rewritten = appended.clone();
        rewritten[2].hash = "u1-edited".into();
        let lineage = classify(&base, &rewritten);
        assert_eq!(lineage.kind, DivergenceKind::HistoryRewritten);
        assert_eq!(lineage.index, Some(2));
        assert_eq!(lineage.shared_prefix, 2);

        // Truncated history (compaction) is a rewrite inside the prefix.
        let truncated = convo(&["u1"]);
        let lineage = classify(&base, &truncated);
        assert_eq!(lineage.kind, DivergenceKind::HistoryRewritten);
        assert_eq!(lineage.index, Some(3));

        // A chat-protocol system message counts as instructions.
        let chat_base = vec![fp("s", ItemSection::Message, Some("system")), msg("u1")];
        let chat_new = vec![fp("s2", ItemSection::Message, Some("system")), msg("u1")];
        assert_eq!(
            classify(&chat_base, &chat_new).kind,
            DivergenceKind::InstructionsChanged
        );

        // Nothing in common at all: a new conversation, not a rewrite.
        let unrelated = vec![fp("other-sys", ItemSection::Instructions, None), msg("z")];
        let lineage = classify(&base, &unrelated);
        assert_eq!(lineage.kind, DivergenceKind::NewChain);
        assert!(!lineage.cache_bust);
        // A changed system prompt with the same tools and history is a
        // divergence at index 0 with an empty shared prefix, never a new chain.
        let mut resent = base.clone();
        resent[0].hash = "sys-v2".into();
        let lineage = classify(&base, &resent);
        assert_eq!(lineage.kind, DivergenceKind::InstructionsChanged);
        assert_eq!(lineage.shared_prefix, 0);
        assert_eq!(lineage.index, Some(0));
        assert!(lineage.cache_bust);
        // Same tools, different system prompt AND different history: a
        // sub-agent conversation, not a rewrite of this one.
        let mut sub_agent = convo(&["task"]);
        sub_agent[0].hash = "agent-sys".into();
        assert_eq!(classify(&base, &sub_agent).kind, DivergenceKind::NewChain);
        assert_eq!(classify(&[], &base).kind, DivergenceKind::Append);
    }

    #[test]
    fn declared_session_extends_its_chain_and_flags_cache_busts() {
        let linker = SessionLinker::new();
        let id = identity("claude-code", Some("s-1"));
        let first = link(&linker, "r1", &id, &convo(&["u1"]), 10);
        assert_eq!(first.lineage.kind, DivergenceKind::NewChain);
        assert_eq!(first.chain_parent_request_id, None);
        assert_eq!(first.upserts.len(), 1);
        let node = &first.upserts[0];
        assert_eq!(node.kind, KIND_DECLARED);
        assert_eq!(node.external_id.as_deref(), Some("s-1"));
        assert_eq!(node.depth, 0);
        assert_eq!(node.request_count, 1);
        assert_eq!(node.root_request_id.as_deref(), Some("r1"));
        assert_eq!(node.client_label.as_deref(), Some("key-abc"));

        let second = link(&linker, "r2", &id, &convo(&["u1", "a1", "u2"]), 20);
        assert_eq!(second.session_id, first.session_id);
        assert_eq!(second.lineage.kind, DivergenceKind::Append);
        assert_eq!(second.chain_parent_request_id.as_deref(), Some("r1"));
        assert_eq!(second.upserts[0].request_count, 2);
        assert_eq!(second.upserts[0].last_seen_ms, 20);

        let mut busted = convo(&["u1", "a1", "u2", "a2", "u3"]);
        busted[1].hash = "tool-b".into();
        let third = link(&linker, "r3", &id, &busted, 30);
        assert_eq!(third.session_id, first.session_id);
        assert_eq!(third.lineage.kind, DivergenceKind::ToolsChanged);
        assert!(third.lineage.cache_bust);
        assert_eq!(third.chain_parent_request_id.as_deref(), Some("r2"));
        assert_eq!(linker.node_count(), 1);
    }

    #[test]
    fn fingerprints_use_the_identity_hash_not_the_storage_hash() {
        let item = crate::content_store::SplitItem {
            ordinal: 0,
            section: ItemSection::Message,
            kind: Some("user".into()),
            hash: "storage".into(),
            identity: "identity".into(),
            canonical: "{}".into(),
        };
        assert_eq!(ItemFingerprint::from(&item).hash, "identity");
    }

    #[test]
    fn a_request_that_is_a_prefix_of_its_predecessor_keeps_the_cache() {
        // oh-my-pi sends no session id: a new conversation from the same
        // system prompt and first message is a prefix of the previous chain.
        let base = convo(&["u1", "a1", "u2", "a2"]);
        let rewound = convo(&["u1"]);
        let lineage = classify(&base, &rewound);
        assert_eq!(lineage.kind, DivergenceKind::HistoryRewritten);
        assert_eq!(lineage.shared_prefix, 3);
        assert!(!lineage.cache_bust, "the prefix it sends is still cached");
    }

    #[test]
    fn a_side_call_sharing_one_message_is_not_a_rewrite_of_the_main_chain() {
        // opencode's title request reuses the user's message verbatim; with a
        // different system prompt and no tools it must not be classified as
        // the main conversation with changed instructions (a false cache
        // bust). It opens its own conversation node instead.
        let linker = SessionLinker::new();
        let id = identity("opencode", None);
        let main = link(&linker, "r1", &id, &convo(&["u1", "a1", "u2"]), 10);
        let side = vec![fp("title-sys", ItemSection::Instructions, None), msg("u1")];
        let title = link(&linker, "r2", &id, &side, 11);
        assert_ne!(title.session_id, main.session_id);
        assert_eq!(title.lineage.kind, DivergenceKind::NewChain);
        assert!(!title.lineage.cache_bust);
        // The main conversation continuing afterwards still lands on its own
        // node (it shares more conversation messages with its own head than
        // the two-item side call does).
        let next = link(
            &linker,
            "r3",
            &id,
            &convo(&["u1", "a1", "u2", "a2", "u3"]),
            12,
        );
        assert_eq!(next.session_id, main.session_id);
        assert_eq!(next.lineage.kind, DivergenceKind::Append);
        assert_eq!(next.chain_parent_request_id.as_deref(), Some("r1"));
    }

    #[test]
    fn a_larger_conversation_is_not_a_sub_session_of_a_side_call() {
        // Claude Code fires a two-item "title" request alongside the first real
        // turn; the declared session owns both chains (no inferred children).
        let linker = SessionLinker::new();
        let id = identity("claude-code", Some("s-1"));
        let mut side = vec![fp("title-sys", ItemSection::Instructions, None)];
        side.push(msg("title-prompt"));
        let title = link(&linker, "r1", &id, &side, 10);
        let main = link(&linker, "r2", &id, &convo(&["u1"]), 11);
        assert_eq!(main.session_id, title.session_id);
        assert_eq!(main.lineage.kind, DivergenceKind::NewChain);
        assert!(!main.lineage.cache_bust);
        assert_eq!(linker.node_count(), 1);
        // The conversation then continues as a plain append on the same chain.
        let next = link(&linker, "r3", &id, &convo(&["u1", "a1", "u2"]), 12);
        assert_eq!(next.session_id, title.session_id);
        assert_eq!(next.lineage.kind, DivergenceKind::Append);
        assert_eq!(next.chain_parent_request_id.as_deref(), Some("r2"));
    }

    #[test]
    fn an_unrelated_conversation_inside_a_declared_session_stays_on_its_node() {
        // Sub-sessions are never inferred: a request inside a declared
        // session that matches no chain starts a new chain on the session's
        // own node, chained after the previous head.
        let linker = SessionLinker::new();
        let id = identity("claude-code", Some("s-1"));
        let main = link(&linker, "r1", &id, &convo(&["u1", "a1", "u2"]), 10);
        let mut agent_items = convo(&["agent-task"]);
        agent_items[0].hash = "agent-sys".into();
        let sub = link(&linker, "r2", &id, &agent_items, 11);
        assert_eq!(sub.session_id, main.session_id);
        assert_eq!(sub.lineage.kind, DivergenceKind::NewChain);
        assert!(!sub.lineage.cache_bust);
        assert_eq!(sub.chain_parent_request_id.as_deref(), Some("r1"));
        assert_eq!(linker.node_count(), 1);
        // The main thread continues after the side conversation, still on the
        // session node, classified against the current head.
        let main_next = link(
            &linker,
            "r4",
            &id,
            &convo(&["u1", "a1", "u2", "a2", "u3"]),
            13,
        );
        assert_eq!(main_next.session_id, main.session_id);
    }

    #[test]
    fn declared_sub_sessions_nest_under_their_declared_parents() {
        let linker = SessionLinker::new();
        // Codex: the root thread, then a spawned thread naming the session as
        // parent, then a nested thread naming the spawned thread.
        let root = identity("codex", Some("s-1"));
        let main = link(&linker, "r1", &root, &convo(&["u1"]), 10);
        let mut spawned = identity("codex", Some("s-1"));
        spawned.sub_session_id = Some("t-2".to_string());
        spawned.session_kind = Some("collab_spawn".to_string());
        let child = link(&linker, "r2", &spawned, &convo(&["task"]), 11);
        assert_ne!(child.session_id, main.session_id);
        let child_row = child
            .upserts
            .iter()
            .find(|row| row.id == child.session_id)
            .unwrap();
        assert_eq!(child_row.kind, KIND_DECLARED);
        assert_eq!(child_row.external_id.as_deref(), Some("t-2"));
        assert_eq!(
            child_row.parent_id.as_deref(),
            Some(main.session_id.as_str())
        );
        assert_eq!(child_row.depth, 1);
        assert_eq!(child_row.session_kind.as_deref(), Some("collab_spawn"));

        let mut nested = identity("codex", Some("s-1"));
        nested.sub_session_id = Some("t-3".to_string());
        nested.parent_session_id = Some("t-2".to_string());
        let grandchild = link(&linker, "r3", &nested, &convo(&["sub-task"]), 12);
        let row = grandchild
            .upserts
            .iter()
            .find(|row| row.id == grandchild.session_id)
            .unwrap();
        assert_eq!(row.parent_id.as_deref(), Some(child.session_id.as_str()));
        assert_eq!(row.depth, 2);
        assert_eq!(linker.node_count(), 3);
    }

    #[test]
    fn pi_agent_child_process_declares_its_parent_session() {
        let linker = SessionLinker::new();
        let parent = identity("pi-agent", Some("p-1"));
        let main = link(&linker, "r1", &parent, &convo(&["u1"]), 10);
        let mut child = identity("pi-agent", Some("c-1"));
        child.parent_session_id = Some("p-1".to_string());
        child.session_kind = Some("subagent".to_string());
        let sub = link(&linker, "r2", &child, &convo(&["task"]), 11);
        let row = sub
            .upserts
            .iter()
            .find(|row| row.id == sub.session_id)
            .unwrap();
        assert_eq!(row.parent_id.as_deref(), Some(main.session_id.as_str()));
        assert_eq!(row.external_id.as_deref(), Some("c-1"));
        assert_eq!(row.depth, 1);

        // A child seen before its parent gets a placeholder parent that the
        // parent's own first request later fills in.
        let linker = SessionLinker::new();
        let sub = link(&linker, "r1", &child, &convo(&["task"]), 10);
        assert_eq!(sub.upserts.len(), 2, "placeholder parent + child");
        let placeholder = sub
            .upserts
            .iter()
            .find(|row| row.external_id.as_deref() == Some("p-1"))
            .unwrap();
        assert_eq!(placeholder.request_count, 0);
        let main = link(&linker, "r2", &parent, &convo(&["u1"]), 11);
        assert_eq!(main.session_id, placeholder.id);
        assert_eq!(main.upserts[0].request_count, 1);
    }

    #[test]
    fn undeclared_requests_content_match_into_conversation_nodes() {
        let linker = SessionLinker::new();
        let id = identity("generic", None);
        let first = link(&linker, "r1", &id, &convo(&["u1"]), 10);
        let row = first
            .upserts
            .iter()
            .find(|row| row.id == first.session_id)
            .unwrap();
        assert_eq!(row.kind, KIND_INFERRED);
        assert_eq!(row.external_id, None);
        assert_eq!(row.parent_id, None);
        assert_eq!(row.client_label.as_deref(), Some("key-abc"));
        let cont = link(&linker, "r2", &id, &convo(&["u1", "a1", "u2"]), 11);
        assert_eq!(cont.session_id, first.session_id);
        assert_eq!(cont.lineage.kind, DivergenceKind::Append);
        // A conversation with nothing in common opens a SEPARATE node.
        let mut other = convo(&["v1"]);
        other[0].hash = "other".into();
        let second_convo = link(&linker, "r3", &id, &other, 12);
        assert_ne!(second_convo.session_id, first.session_id);
        assert_eq!(
            second_convo
                .upserts
                .iter()
                .find(|row| row.id == second_convo.session_id)
                .unwrap()
                .parent_id,
            None,
            "a new conversation node has no parent"
        );
    }

    #[test]
    fn production_bug_regression_same_client_unrelated_conversations_stay_separate() {
        // The production failure this rewrite fixes (oh-my-pi against the
        // chat-completions ingress): one client sent thousands of unrelated
        // conversations sharing the same system prompt and tool list. The
        // old scoring chained every new conversation onto the previous one's
        // node (6356 requests on a single session). Shared instructions and
        // tools alone are NEVER a match now.
        let linker = SessionLinker::new();
        let id = identity("oh-my-pi", None);
        let first = link(&linker, "r1", &id, &convo(&["u1"]), 10);
        let second = link(&linker, "r2", &id, &convo(&["v1"]), 11);
        assert_ne!(
            second.session_id, first.session_id,
            "unrelated conversation ⇒ its own node, never the previous one"
        );
        assert_eq!(second.lineage.kind, DivergenceKind::NewChain);
        assert_eq!(second.chain_parent_request_id, None);
        assert!(!second.lineage.cache_bust);
        assert_eq!(linker.node_count(), 2);

        // Each conversation then continues on its own node.
        let first_next = link(&linker, "r3", &id, &convo(&["u1", "a1", "u2"]), 12);
        assert_eq!(first_next.session_id, first.session_id);
        let second_next = link(&linker, "r4", &id, &convo(&["v1", "a1", "v2"]), 13);
        assert_eq!(second_next.session_id, second.session_id);
        assert_eq!(linker.node_count(), 2);
    }

    #[test]
    fn undeclared_conversations_never_cross_clients() {
        let linker = SessionLinker::new();
        let id = identity("generic", None);
        let a = link_with_client(&linker, "r1", &id, "key-a", &convo(&["u1"]), 10);
        // Identical messages from a different client: no shared candidates.
        let b = link_with_client(&linker, "r2", &id, "key-b", &convo(&["u1"]), 11);
        assert_ne!(a.session_id, b.session_id);
        // The same client's continuation still finds its node.
        let a2 = link_with_client(&linker, "r3", &id, "key-a", &convo(&["u1", "a1"]), 12);
        assert_eq!(a2.session_id, a.session_id);
    }

    #[test]
    fn seeding_restores_a_declared_session_and_its_head() {
        let linker = SessionLinker::new();
        let row = SessionRow {
            display_number: None,
            id: "node-1".to_string(),
            parent_id: None,
            kind: KIND_DECLARED.to_string(),
            harness: "claude-code".to_string(),
            harness_version: None,
            external_id: Some("s-1".to_string()),
            session_kind: None,
            client_label: None,
            virtual_key_id: None,
            user_id: None,
            depth: 0,
            root_request_id: Some("old-1".to_string()),
            spawned_by_request_id: None,
            first_seen_ms: 1,
            last_seen_ms: 2,
            request_count: 5,
        };
        assert!(!linker.knows_declared("claude-code", "s-1"));
        linker.seed(vec![(
            row,
            Some(ChainHead {
                request_id: "old-1".to_string(),
                items: convo(&["u1"]),
            }),
        )]);
        assert!(linker.knows_declared("claude-code", "s-1"));
        let id = identity("claude-code", Some("s-1"));
        let next = link(&linker, "r1", &id, &convo(&["u1", "a1", "u2"]), 10);
        assert_eq!(next.session_id, "node-1");
        assert_eq!(next.lineage.kind, DivergenceKind::Append);
        assert_eq!(next.chain_parent_request_id.as_deref(), Some("old-1"));
        assert_eq!(next.upserts[0].request_count, 6);
    }

    #[test]
    fn seeding_restores_conversation_candidates_for_content_matching() {
        // The warm-up path: a process restart seeds the client's recent
        // conversation nodes, and turn 2 of a conversation still matches.
        let linker = SessionLinker::new();
        let row = SessionRow {
            display_number: None,
            id: "conv-1".to_string(),
            parent_id: None,
            kind: KIND_INFERRED.to_string(),
            harness: "generic".to_string(),
            harness_version: None,
            external_id: None,
            session_kind: None,
            client_label: Some("key-abc".to_string()),
            virtual_key_id: None,
            user_id: None,
            depth: 0,
            root_request_id: Some("old-1".to_string()),
            spawned_by_request_id: None,
            first_seen_ms: 1,
            last_seen_ms: 2,
            request_count: 1,
        };
        linker.seed(vec![(
            row,
            Some(ChainHead {
                request_id: "old-1".to_string(),
                items: convo(&["u1"]),
            }),
        )]);
        assert!(linker.knows_conversations("generic", Some("key-abc")));
        let id = identity("generic", None);
        let next = link(&linker, "r1", &id, &convo(&["u1", "a1", "u2"]), 10);
        assert_eq!(next.session_id, "conv-1");
        assert_eq!(next.lineage.kind, DivergenceKind::Append);
        assert_eq!(next.chain_parent_request_id.as_deref(), Some("old-1"));
        // And an unrelated conversation still opens its own node.
        let mut other = convo(&["v1"]);
        other[0].hash = "other".into();
        let unrelated = link(&linker, "r2", &id, &other, 11);
        assert_ne!(unrelated.session_id, "conv-1");
    }

    #[test]
    fn eviction_keeps_the_index_bounded() {
        let linker = SessionLinker::new();
        for n in 0..(MAX_NODES + 50) {
            let id = identity("generic", Some(&format!("s-{n}")));
            link(&linker, "r", &id, &convo(&["u"]), n as i64);
        }
        assert!(linker.node_count() <= MAX_NODES);
        assert!(!linker.knows_declared("generic", "s-0"));
        assert!(linker.knows_declared("generic", &format!("s-{}", MAX_NODES + 49)));
    }
}
