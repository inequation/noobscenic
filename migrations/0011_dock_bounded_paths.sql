-- A clean can use several `pathID` values. In the lean build, paths are retained
-- by dock-bounded run rather than by individual path id: the current run and the
-- run immediately before it survive, which is the last completed path plus any
-- path being collected now.
ALTER TABLE clean_paths ADD COLUMN run_id INTEGER NOT NULL DEFAULT 0;

CREATE INDEX clean_paths_sn_run_ms ON clean_paths(sn, run_id, updated_ms);

-- One row per device. `current_run_id` starts at zero for paths written before the
-- first observed dock event. Each transition into `charge`/`fullcharge` advances it;
-- `previous_run_id`, `last_dock_ms` and `previous_dock_ms` identify the two paths
-- that make up the human-facing dock-bounded path.
CREATE TABLE path_state (
    sn               TEXT PRIMARY KEY,
    docked           INTEGER NOT NULL DEFAULT 0,
    current_run_id   INTEGER NOT NULL DEFAULT 0,
    previous_run_id  INTEGER,
    last_dock_ms     INTEGER,
    previous_dock_ms INTEGER
);
