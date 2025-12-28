-- Postgres NOTIFY trigger for notifications
-- This wakes up the Rust worker immediately when a notification is inserted

-- Function that sends NOTIFY signal
CREATE OR REPLACE FUNCTION activity.fn_notification_inserted()
RETURNS TRIGGER AS $$
BEGIN
    -- Send notification with the id as payload
    PERFORM pg_notify('notify_event', NEW.id::text);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Drop existing trigger if exists
DROP TRIGGER IF EXISTS trg_notification_inserted ON activity.notifications;

-- Create trigger on INSERT
CREATE TRIGGER trg_notification_inserted
    AFTER INSERT ON activity.notifications
    FOR EACH ROW
    EXECUTE FUNCTION activity.fn_notification_inserted();

COMMENT ON FUNCTION activity.fn_notification_inserted() IS
    'Sends pg_notify signal when notification is inserted, waking up the Rust worker';
