-- r[operation.cancel] A cancel request issued via /apps/action/cancel
-- must survive a daemon restart so the replay resumes into a
-- pre-cancelled state rather than re-executing from scratch.
-- 0 = no cancel pending; 1 = cancel requested.
ALTER TABLE current_operation
    ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0;
