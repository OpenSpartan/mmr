//! What reaches the decoder, from a capture or from the proxy: one lobby WebSocket
//! message from the server, or one Xbox session document.

use crate::amqp;
use crate::tracker::{Event, Tracker};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    WebSocket,
    Document,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: Kind,
    /// The connection it came on, numbered by the proxy or the capture.
    pub conn: u64,
    pub at: String,
    pub payload: Vec<u8>,
}

/// Decodes items into a tracker, the same way for every input.
#[derive(Default)]
pub struct Pipeline {
    pub decoder: amqp::Decoder,
    pub tracker: Tracker,
    pub rosters: u64,
    pub documents: u64,
    pub bad_documents: u64,
    /// The match the last item's document went to, if it went to one.
    pub doc_match: Option<String>,
    /// Where saved matches are, so a restart continues them.
    pub resume_from: Option<std::path::PathBuf>,
}

impl Pipeline {
    fn resume(&mut self, key: &str) {
        let Some(dir) = &self.resume_from else { return };
        if self.tracker.has(key) {
            return;
        }
        match crate::store::load(dir, key) {
            Ok(Some(m)) => self.tracker.resume(m),
            Ok(None) => {}
            Err(e) => *self.decoder.problems.entry(format!("can't continue a saved match: {e}")).or_default() += 1,
        }
    }

    /// What this item changed, in order.
    pub fn item(&mut self, it: &Item) -> Vec<Event> {
        let mut events = Vec::new();
        self.doc_match = None;
        match it.kind {
            Kind::WebSocket => {
                for m in self.decoder.feed(it.conn, &it.at, &it.payload) {
                    self.rosters += 1;
                    let key = self.tracker.key_for(&m.session, &m.roster);
                    self.resume(&key);
                    events.extend(self.tracker.apply(&m.session, m.owner, &m.at, &m.roster));
                }
            }
            Kind::Document => {
                let Ok(doc) = serde_json::from_slice::<serde_json::Value>(&it.payload) else {
                    self.bad_documents += 1;
                    return events;
                };
                if let Some(s) = crate::tracker::document_session(&doc) {
                    self.resume(&s);
                }
                // Lobby and party documents have no game session, so no match takes them.
                if let Some((key, ev)) = self.tracker.apply_document(&it.at, &doc) {
                    self.documents += 1;
                    self.doc_match = Some(key);
                    events.extend(ev);
                }
            }
        }
        events
    }
}
