-- doc/PLAN.md §15 phase 6: the binding state machine. The robot's BindUser flow
-- sends its preBind the moment channel B is up and treats plain `code:0` as
-- "directly bind success" (PROTOCOL §A "Gap 2"), so we record the transitions to
-- make the state visible and retries recognisable.
ALTER TABLE devices ADD COLUMN bind_user  TEXT;
ALTER TABLE devices ADD COLUMN bound_ms   INTEGER;
ALTER TABLE devices ADD COLUMN unbound_ms INTEGER;
