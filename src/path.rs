//! Clean-path assembly (`infoType` 21011) — doc/PLAN.md §11, FUNC_MAP.md §3.
//!
//! The path arrives as an HTTP `cleanPack/response` to a cloud request. The reply's
//! `posArray` holds `[x,y]` pairs in **millimetres** with the low 2 bits of each value
//! used as a point-type tag (strip with `& !3` to draw). Chunks carry `startPos` so a
//! long path can arrive in pieces; assembly is keyed by `(sn, pathID)` and is tolerant
//! of duplicates and out-of-order chunks.

use serde_json::Value;

use crate::error::Result;

/// One 21011 reply's payload.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub path_id: i64,
    pub start_pos: usize,
    pub total_points: usize,
    pub user_id: Option<String>,
    pub points: Vec<[f64; 2]>,
}

pub fn parse(data: &Value) -> Result<Chunk> {
    let path_id = data
        .get("pathID")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let start_pos = data
        .get("startPos")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0) as usize;
    let total_points = data
        .get("totalPoints")
        .and_then(Value::as_i64)
        .unwrap_or_default()
        .max(0) as usize;
    let user_id = data
        .get("userId")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut points = Vec::new();
    if let Some(array) = data.get("posArray").and_then(Value::as_array) {
        for pair in array {
            let Some(pair) = pair.as_array() else {
                continue;
            };
            let (Some(x), Some(y)) = (
                pair.first().and_then(Value::as_f64),
                pair.get(1).and_then(Value::as_f64),
            ) else {
                continue;
            };
            points.push([x, y]);
        }
    }
    // An empty `posArray` with a header is legal: a request whose `startPos` is at
    // or past the path's end gets "nothing new" back, which is exactly what the
    // live path poller sees between the robot's moves (FUNC_MAP.md §3).
    Ok(Chunk {
        path_id,
        start_pos,
        total_points,
        user_id,
        points,
    })
}

/// Sparse path assembly: `points[i]` is filled once the chunk covering point `i`
/// arrives, in any order and any number of times.
#[derive(Debug, Default)]
pub struct Assembly {
    points: Vec<Option<[f64; 2]>>,
}

impl Assembly {
    pub fn new() -> Assembly {
        Assembly::default()
    }

    /// Rebuild from the stored `points_json` (`[x,y]` or `null` per point slot).
    pub fn from_json(json: &str) -> Assembly {
        let points = serde_json::from_str::<Vec<Option<[f64; 2]>>>(json).unwrap_or_default();
        Assembly { points }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.points).expect("points always serialise")
    }

    /// Merge one chunk; returns how many points it filled. Later chunks win on
    /// overlap (the robot re-sends from `startPos`, so a correction must be kept).
    pub fn merge(&mut self, chunk: &Chunk) -> usize {
        let needed = (chunk.start_pos + chunk.points.len()).max(chunk.total_points);
        if self.points.len() < needed {
            self.points.resize(needed, None);
        }
        let mut filled = 0;
        for (index, point) in chunk.points.iter().enumerate() {
            let slot = chunk.start_pos + index;
            if let Some(entry) = self.points.get_mut(slot) {
                if entry.is_none() {
                    filled += 1;
                }
                *entry = Some(*point);
            }
        }
        filled
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn filled(&self) -> usize {
        self.points.iter().filter(|point| point.is_some()).count()
    }

    /// The next `startPos` to ask the robot from: the first point we do not have,
    /// or the end of the path when it is contiguous. This is what recovers a new
    /// `pathID` (whose first reply is a header-only chunk with every slot `None`)
    /// and backfills any hole a merge left behind.
    pub fn next_index(&self) -> usize {
        self.points
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.points.len())
    }

    pub fn is_complete(&self) -> bool {
        !self.points.is_empty() && self.points.iter().all(Option::is_some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chunk(start: usize, total: usize, points: &[[f64; 2]]) -> Chunk {
        Chunk {
            path_id: 7,
            start_pos: start,
            total_points: total,
            user_id: Some("u".into()),
            points: points.to_vec(),
        }
    }

    #[test]
    fn parses_the_documented_reply() {
        // `dispatch` hands this parser the message's `data` member, not the whole
        // `cleanPack/response` message.
        let parsed = parse(&json!({
            "userId": "u", "pathID": 12, "startPos": 4, "totalPoints": 10,
            "posArray": [[1004, 2000], [1008, 2004]], "pointCounts": 2,
        }))
        .unwrap();
        assert_eq!(parsed.path_id, 12);
        assert_eq!(parsed.start_pos, 4);
        assert_eq!(parsed.total_points, 10);
        assert_eq!(parsed.points, vec![[1004.0, 2000.0], [1008.0, 2004.0]]);
    }

    #[test]
    fn out_of_order_chunks_fill_in_and_complete() {
        let mut assembly = Assembly::new();
        assert_eq!(
            assembly.merge(&chunk(2, 4, &[[30.0, 31.0], [40.0, 41.0]])),
            2
        );
        assert!(!assembly.is_complete());
        assert_eq!(assembly.to_json(), "[null,null,[30.0,31.0],[40.0,41.0]]");
        assert_eq!(
            assembly.merge(&chunk(0, 4, &[[10.0, 11.0], [20.0, 21.0]])),
            2
        );
        assert!(assembly.is_complete());
        assert_eq!(assembly.filled(), 4);
    }

    #[test]
    fn duplicate_chunks_are_idempotent_and_corrections_win() {
        let mut assembly = Assembly::new();
        assembly.merge(&chunk(0, 2, &[[1.0, 2.0], [3.0, 4.0]]));
        assert_eq!(
            assembly.merge(&chunk(0, 2, &[[1.0, 2.0], [3.0, 4.0]])),
            0,
            "duplicate fills nothing new"
        );
        assert_eq!(assembly.merge(&chunk(1, 2, &[[9.0, 9.0]])), 0);
        assert_eq!(assembly.to_json(), "[[1.0,2.0],[9.0,9.0]]");
    }

    #[test]
    fn storage_round_trips() {
        let mut assembly = Assembly::new();
        assembly.merge(&chunk(1, 3, &[[5.0, 6.0]]));
        let reloaded = Assembly::from_json(&assembly.to_json());
        assert_eq!(reloaded.len(), 3);
        assert_eq!(reloaded.filled(), 1);
        assert!(!reloaded.is_complete());
    }

    #[test]
    fn next_index_points_at_the_first_hole() {
        let mut assembly = Assembly::new();
        assert_eq!(
            assembly.next_index(),
            0,
            "nothing stored: ask from the start"
        );
        assembly.merge(&chunk(0, 4, &[[1.0, 1.0], [2.0, 2.0]]));
        assert_eq!(
            assembly.next_index(),
            2,
            "a contiguous prefix asks from its end"
        );
        assembly.merge(&chunk(3, 4, &[[4.0, 4.0]]));
        assert_eq!(assembly.next_index(), 2, "a hole is asked for first");
    }
}
