-- Postgres NOTIFY trigger for new notifications
-- This wakes up the Rust worker immediately when a notification is inserted

-- Function that sends NOTIFY signal
CREATE OR REPLACE FUNCTION activity.notify_new_notification()
RETURNS TRIGGER AS $$
BEGIN
    -- Send notification with the notification_id as payload
    PERFORM pg_notify('notify_event', NEW.notification_id::text);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Drop existing trigger if exists
DROP TRIGGER IF EXISTS trg_notify_new_notification ON activity.notifications;

-- Create trigger on INSERT
CREATE TRIGGER trg_notify_new_notification
    AFTER INSERT ON activity.notifications
    FOR EACH ROW
    EXECUTE FUNCTION activity.notify_new_notification();

COMMENT ON FUNCTION activity.notify_new_notification() IS
    'Sends pg_notify signal when new notification is inserted, waking up the Rust worker';
