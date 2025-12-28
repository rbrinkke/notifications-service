-- ============================================================================
-- CLEANUP: Remove unused notification database objects
-- Executed: 2025-12-27
--
-- The notifications-service (Rust worker) only uses:
--   - Tables: activity.notifications, activity.user_devices
--   - Functions: sp_notification_success(), sp_notification_failure()
--   - Trigger: fn_notification_inserted() (for pg_notify)
--
-- Everything else was legacy from the old Python API and has been removed.
-- ============================================================================

-- Drop old triggers first (before dropping functions they depend on)
DROP TRIGGER IF EXISTS trg_notification_sync ON activity.notifications;
DROP TRIGGER IF EXISTS trg_notification_sync ON activity.notifications_new;
DROP TRIGGER IF EXISTS trg_notify_new_notification ON activity.notifications;

-- Drop legacy functions with "new" in name
DROP FUNCTION IF EXISTS activity.notify_new_notification() CASCADE;

-- Drop unused stored procedures/functions (various signatures)
DROP FUNCTION IF EXISTS activity.sp_create_notification_v2(
    uuid, uuid, varchar, varchar, uuid, varchar, text, jsonb, varchar,
    integer, integer, varchar, varchar, varchar, jsonb, varchar, timestamptz
);
DROP FUNCTION IF EXISTS activity.sp_create_notification(uuid, uuid, varchar, varchar, uuid, varchar, text, jsonb);
DROP FUNCTION IF EXISTS activity.sp_get_user_notifications(uuid, varchar, varchar, integer, integer, boolean);
DROP FUNCTION IF EXISTS activity.sp_mark_notification_as_read(uuid, uuid);
DROP FUNCTION IF EXISTS activity.sp_mark_notifications_as_read_bulk(uuid, uuid[], varchar);
DROP FUNCTION IF EXISTS activity.sp_bulk_delete_notifications(uuid, uuid[], boolean);
DROP FUNCTION IF EXISTS activity.fn_notify_notification_sync() CASCADE;
DROP FUNCTION IF EXISTS activity.sp_get_notification_settings(uuid);
DROP FUNCTION IF EXISTS activity.sp_update_notification_settings(uuid, boolean, boolean, boolean, jsonb, time, time);
DROP FUNCTION IF EXISTS activity.sp_get_grouped_notification_actors(uuid, integer);
DROP FUNCTION IF EXISTS activity.sp_cleanup_expired_notification_events();
DROP FUNCTION IF EXISTS activity.sp_delete_notification(uuid, uuid, boolean);
DROP FUNCTION IF EXISTS activity.sp_get_notification_by_id(uuid, uuid);
DROP FUNCTION IF EXISTS activity.sp_record_notification_event(uuid, varchar, uuid, jsonb, integer, boolean);

-- Drop unused tables
DROP TABLE IF EXISTS activity.notifications_new CASCADE;
DROP TABLE IF EXISTS activity.notification_events CASCADE;
DROP TABLE IF EXISTS activity.notification_preferences CASCADE;
DROP TABLE IF EXISTS activity.notification_settings CASCADE;

-- ============================================================================
-- REMAINING OBJECTS (used by notifications-service):
--   - Table: activity.notifications
--   - Table: activity.user_devices
--   - Function: activity.sp_notification_success(uuid)
--   - Function: activity.sp_notification_failure(uuid, text, integer)
--   - Function: activity.fn_notification_inserted() [trigger]
--   - Trigger: trg_notification_inserted ON activity.notifications
-- ============================================================================
