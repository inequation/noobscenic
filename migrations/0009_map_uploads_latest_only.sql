-- The live grid table is a cache, not an archive: keep only the newest row per
-- device. Every reader orders by received_ms DESC, and the NBMP backup slot
-- (map_backups, BACKUP_MAP.md) is the archival mechanism.
DELETE FROM map_uploads
WHERE EXISTS (
    SELECT 1 FROM map_uploads newer
    WHERE newer.sn = map_uploads.sn
      AND (
          newer.received_ms > map_uploads.received_ms
          OR (newer.received_ms = map_uploads.received_ms AND newer.id > map_uploads.id)
      )
);
