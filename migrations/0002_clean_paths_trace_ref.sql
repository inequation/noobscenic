-- Phase 4: every stored row must reference the trace it came from (doc/PLAN.md §11).

ALTER TABLE clean_paths ADD COLUMN trace_ref TEXT;
