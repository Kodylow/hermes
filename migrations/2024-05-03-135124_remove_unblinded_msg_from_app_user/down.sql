-- This file should undo anything in `up.sql`
ALTER TABLE app_user
ADD COLUMN unblinded_msg VARCHAR(255) NOT NULL UNIQUE;
