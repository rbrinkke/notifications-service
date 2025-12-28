import psycopg2
import time
import uuid
import sys
import os
from datetime import datetime, timedelta

# Configuration
DB_DSN = "postgres://postgres:postgres_secure_password_change_in_prod@localhost:5441/activitydb"

# ANSI Colors
class Colors:
    HEADER = '\033[95m'
    OKBLUE = '\033[94m'
    OKGREEN = '\033[92m'
    WARNING = '\033[93m'
    FAIL = '\033[91m'
    ENDC = '\033[0m'
    BOLD = '\033[1m'

def log(msg, color=Colors.OKBLUE):
    print(f"{color}[TEST]{Colors.ENDC} {msg}")

def get_db():
    try:
        return psycopg2.connect(DB_DSN)
    except Exception as e:
        print(f"{Colors.FAIL}Failed to connect to DB at {DB_DSN}{Colors.ENDC}")
        print(e)
        sys.exit(1)

def wait_for_processing(conn, notif_id, timeout=10):
    start = time.time()
    while time.time() - start < timeout:
        with conn.cursor() as cur:
            cur.execute("SELECT is_processed, error_count, last_error FROM activity.notifications WHERE id = %s", (str(notif_id),))
            row = cur.fetchone()
            if row and row[0] is True:
                return True, row
            time.sleep(0.5)
    return False, row

def run_tests():
    conn = get_db()
    conn.autocommit = True
    
    print(f"{Colors.HEADER}════════════════════════════════════════════════════════════{Colors.ENDC}")
    print(f"{Colors.HEADER}  NOTIFICATIONS SERVICE - END-TO-END TEST SUITE {Colors.ENDC}")
    print(f"{Colors.HEADER}════════════════════════════════════════════════════════════{Colors.ENDC}")

    # ---------------------------------------------------------
    # TEST 1: Instant Notification
    # ---------------------------------------------------------
    log("Test 1: Instant Notification Delivery", Colors.BOLD)
    notif_id = uuid.uuid4()
    user_id = uuid.uuid4() # Random user
    
    with conn.cursor() as cur:
        cur.execute("""
            INSERT INTO activity.notifications (id, user_id, title, message, notification_type, priority)
            VALUES (%s, %s, 'E2E Test', 'Instant delivery test', 'system', 'high')
        """, (str(notif_id), str(user_id)))
    
    log(f"  -> Inserted notification {notif_id}")
    
    success, row = wait_for_processing(conn, notif_id)
    if success:
        log(f"  -> ✅ Processed successfully within timeout", Colors.OKGREEN)
    else:
        log(f"  -> ❌ Failed or timed out. State: {row}", Colors.FAIL)

    print("")

    # ---------------------------------------------------------
    # TEST 2: Scheduled Notification
    # ---------------------------------------------------------
    log("Test 2: Scheduled Notification (Time Travel)", Colors.BOLD)
    notif_id_sched = uuid.uuid4()
    delay_sec = 5
    deliver_at = datetime.now() + timedelta(seconds=delay_sec)
    
    log(f"  -> Scheduling for {delay_sec} seconds in the future...")
    
    with conn.cursor() as cur:
        cur.execute("""
            INSERT INTO activity.notifications (id, user_id, title, message, notification_type, deliver_at)
            VALUES (%s, %s, 'E2E Scheduled', 'From the future', 'system', %s)
        """, (str(notif_id_sched), str(user_id), deliver_at))

    # Immediate check - should be False
    with conn.cursor() as cur:
        cur.execute("SELECT is_processed FROM activity.notifications WHERE id = %s", (str(notif_id_sched),))
        processed = cur.fetchone()[0]
        
    if processed:
        log(f"  -> ❌ Failed: Notification processed too early!", Colors.FAIL)
    else:
        log(f"  -> ✅ Correctly ignored (too early)", Colors.OKGREEN)
        
    log("  -> Waiting for time to pass...")
    time.sleep(delay_sec + 2) # Wait delay + buffer
    
    # Check again
    success, row = wait_for_processing(conn, notif_id_sched, timeout=5)
    if success:
        log(f"  -> ✅ Processed successfully after delay", Colors.OKGREEN)
    else:
        log(f"  -> ❌ Failed to process after scheduled time. State: {row}", Colors.FAIL)

    print("")

    # ---------------------------------------------------------
    # TEST 3: Broadcast (Nil UUID)
    # ---------------------------------------------------------
    log("Test 3: Global Broadcast (Nil UUID)", Colors.BOLD)
    notif_id_broad = uuid.uuid4()
    nil_uuid = "00000000-0000-0000-0000-000000000000"
    
    with conn.cursor() as cur:
        cur.execute("""
            INSERT INTO activity.notifications (id, user_id, title, message, notification_type)
            VALUES (%s, %s, 'E2E Broadcast', 'Testing global reach', 'system')
        """, (str(notif_id_broad), nil_uuid))
        
    log(f"  -> Inserted broadcast {notif_id_broad}")
    
    success, row = wait_for_processing(conn, notif_id_broad)
    if success:
        log(f"  -> ✅ Broadcast processed successfully", Colors.OKGREEN)
    else:
        log(f"  -> ❌ Broadcast failed. State: {row}", Colors.FAIL)

    print(f"{Colors.HEADER}════════════════════════════════════════════════════════════{Colors.ENDC}")
    conn.close()

if __name__ == "__main__":
    run_tests()
