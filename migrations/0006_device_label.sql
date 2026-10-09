-- A user-visible label for the robot. The id the robot reports in its preBind
-- (`bind_user`) is the *cloud account* it is bound to, not a name for the device, so
-- it must not be displayed as one (operator decision, 2026-10-09). The label starts
-- as the serial number and is the only user-editable setting for now.
ALTER TABLE devices ADD COLUMN label TEXT;
UPDATE devices SET label = sn WHERE label IS NULL OR label = '';
