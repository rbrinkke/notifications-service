-- Add error tracking columns to notifications table
ALTER TABLE activity.notifications
ADD COLUMN IF NOT EXISTS error_count INTEGER DEFAULT 0,
ADD COLUMN IF NOT EXISTS last_error TEXT,
ADD COLUMN IF NOT EXISTS last_error_at TIMESTAMP WITH TIME ZONE;

-- Stored procedure: Mark notification as successfully processed
CREATE OR REPLACE FUNCTION activity.sp_notification_success(
    p_notification_id UUID
) RETURNS BOOLEAN AS $$
DECLARE
    v_updated BOOLEAN;
BEGIN
    UPDATE activity.notifications
    SET
        is_processed = true,
        updated_at = now()
    WHERE notification_id = p_notification_id
      AND is_processed = false;

    GET DIAGNOSTICS v_updated = ROW_COUNT;
    RETURN v_updated > 0;
END;
$$ LANGUAGE plpgsql;

-- Stored procedure: Record notification failure
-- Returns true if we should stop retrying (max retries reached)
CREATE OR REPLACE FUNCTION activity.sp_notification_failure(
    p_notification_id UUID,
    p_error_message TEXT,
    p_max_retries INTEGER DEFAULT 3
) RETURNS BOOLEAN AS $$
DECLARE
    v_new_error_count INTEGER;
    v_should_stop BOOLEAN := false;
BEGIN
    UPDATE activity.notifications
    SET
        error_count = error_count + 1,
        last_error = p_error_message,
        last_error_at = now(),
        updated_at = now(),
        -- If we hit max retries, mark as processed to stop the loop
        is_processed = CASE
            WHEN error_count + 1 >= p_max_retries THEN true
            ELSE false
        END
    WHERE notification_id = p_notification_id
    RETURNING error_count, is_processed INTO v_new_error_count, v_should_stop;

    RETURN v_should_stop;
END;
$$ LANGUAGE plpgsql;

-- Index for efficient fetching of unprocessed notifications
CREATE INDEX IF NOT EXISTS idx_notifications_unprocessed
ON activity.notifications (created_at ASC)
WHERE is_processed = false;

COMMENT ON COLUMN activity.notifications.error_count IS 'Number of delivery attempts that failed';
COMMENT ON COLUMN activity.notifications.last_error IS 'Last error message for debugging';
COMMENT ON COLUMN activity.notifications.last_error_at IS 'Timestamp of last failed attempt';
