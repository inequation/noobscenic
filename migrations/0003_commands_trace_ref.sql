-- doc/PLAN.md §8: every `commands` row must be traceable back to the bytes that
-- sent it. The writer task records the outbound frame in the tap and stores the
-- resulting reference here (same pattern as 0002 did for `clean_paths`).
ALTER TABLE commands ADD COLUMN trace_ref TEXT;
