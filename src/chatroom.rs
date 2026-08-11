use std::sync::{Arc, Mutex};

pub const DEFAULT_CAPACITY: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Reasoning,
    Message,
    Activity,
    Observation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub sequence: u64,
    pub seat: String,
    pub kind: Kind,
    pub body: String,
}

#[derive(Debug, Default)]
struct Room {
    entries: Vec<Entry>,
    next: u64,
    capacity: usize,
    dropped: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Chatroom {
    room: Arc<Mutex<Room>>,
}

impl Chatroom {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Chatroom {
            room: Arc::new(Mutex::new(Room {
                entries: Vec::new(),
                next: 1,
                capacity: capacity.max(1),
                dropped: 0,
            })),
        }
    }

    pub fn post(&self, seat: &str, kind: Kind, body: &str) -> Option<u64> {
        let body = body.trim();
        if body.is_empty() {
            return None;
        }
        let mut room = self.room.lock().ok()?;
        let sequence = room.next;
        room.next = room.next.saturating_add(1);
        let capacity = room.capacity;
        room.entries.push(Entry {
            sequence,
            seat: seat.to_string(),
            kind,
            body: body.to_string(),
        });
        if room.entries.len() > capacity {
            let excess = room.entries.len() - capacity;
            room.entries.drain(0..excess);
            room.dropped = room.dropped.saturating_add(excess as u64);
        }
        Some(sequence)
    }

    pub fn entries(&self) -> Vec<Entry> {
        self.room
            .lock()
            .map(|room| room.entries.clone())
            .unwrap_or_default()
    }

    pub fn of_kind(&self, kind: Kind) -> Vec<Entry> {
        self.entries()
            .into_iter()
            .filter(|entry| entry.kind == kind)
            .collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.room.lock().map(|room| room.entries.len()).unwrap_or(0)
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[cfg(test)]
    pub fn dropped(&self) -> u64 {
        self.room.lock().map(|room| room.dropped).unwrap_or(0)
    }

    #[cfg(test)]
    pub fn clear(&self) {
        if let Ok(mut room) = self.room.lock() {
            room.entries.clear();
            room.dropped = 0;
        }
    }

    pub fn observations_for(&self, seat: &str) -> Vec<String> {
        self.of_kind(Kind::Observation)
            .into_iter()
            .filter(|entry| entry.seat != seat)
            .map(|entry| entry.body)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_keep_chronological_sequence_per_seat() {
        let room = Chatroom::new();
        room.post("alpha/one", Kind::Reasoning, "weighing the split");
        room.post("beta/two", Kind::Reasoning, "checking the renderer");
        room.post("alpha/one", Kind::Message, "I will take the scheduler");
        let entries = room.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].sequence, 1);
        assert_eq!(entries[1].sequence, 2);
        assert_eq!(entries[2].sequence, 3);
        assert_eq!(entries[1].seat, "beta/two");
        assert_eq!(entries[2].kind, Kind::Message);
    }

    #[test]
    fn blank_bodies_are_never_posted() {
        let room = Chatroom::new();
        assert!(room.post("alpha/one", Kind::Reasoning, "   ").is_none());
        assert!(room.post("alpha/one", Kind::Reasoning, "\n\n").is_none());
        assert!(room.is_empty());
        assert!(room.post("alpha/one", Kind::Reasoning, " real ").is_some());
        assert_eq!(room.entries()[0].body, "real");
    }

    #[test]
    fn the_room_is_bounded_and_drops_oldest_first() {
        let room = Chatroom::with_capacity(3);
        for index in 0..6 {
            room.post("alpha/one", Kind::Message, &format!("line {index}"));
        }
        let entries = room.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].body, "line 3");
        assert_eq!(entries[2].body, "line 5");
        assert_eq!(room.dropped(), 3);
    }

    #[test]
    fn observations_are_shared_with_the_other_seat_only() {
        let room = Chatroom::new();
        room.post("alpha/one", Kind::Observation, "beta is fast at tests");
        room.post("beta/two", Kind::Observation, "alpha localizes well");
        assert_eq!(
            room.observations_for("alpha/one"),
            vec!["alpha localizes well"]
        );
        assert_eq!(
            room.observations_for("beta/two"),
            vec!["beta is fast at tests"]
        );
    }

    #[test]
    fn concurrent_seats_post_without_loss_or_duplicate_sequence() {
        let room = Chatroom::with_capacity(4096);
        let seats = ["alpha/one", "beta/two", "gamma/three", "delta/four"];
        std::thread::scope(|scope| {
            for seat in seats {
                let room = room.clone();
                scope.spawn(move || {
                    for index in 0..50 {
                        room.post(seat, Kind::Reasoning, &format!("{seat} step {index}"));
                    }
                });
            }
        });
        let entries = room.entries();
        assert_eq!(entries.len(), 200, "no post may be lost under contention");
        let mut sequences: Vec<u64> = entries.iter().map(|entry| entry.sequence).collect();
        sequences.sort_unstable();
        sequences.dedup();
        assert_eq!(sequences.len(), 200, "sequences must be unique");
        for seat in seats {
            assert_eq!(
                entries.iter().filter(|entry| entry.seat == seat).count(),
                50
            );
        }
    }

    #[test]
    fn a_shared_handle_sees_the_same_room() {
        let room = Chatroom::new();
        let peer = room.clone();
        room.post("alpha/one", Kind::Message, "hello");
        assert_eq!(peer.len(), 1);
        peer.post("beta/two", Kind::Message, "hello back");
        assert_eq!(room.len(), 2);
        room.clear();
        assert!(peer.is_empty());
    }
}
