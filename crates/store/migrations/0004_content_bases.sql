-- DevBridge schema v4: ancestor content keys, used to recognise a change made
-- on top of content the receiver already has (avoids spurious conflicts when
-- two devices start from identical files).
ALTER TABLE file_versions ADD COLUMN base_hashes TEXT NOT NULL DEFAULT '[]';
ALTER TABLE transfer_jobs ADD COLUMN base_hashes TEXT NOT NULL DEFAULT '[]';
