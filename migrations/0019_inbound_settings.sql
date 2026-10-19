-- Body-free request lists retain client settings independently of raw capture.
ALTER TABLE requests ADD COLUMN inbound_settings_json TEXT;
