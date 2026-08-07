-- Poly SQL demo — executes against an in-memory SQLite database via
-- `bun:sqlite` exec (schema/migration style). A non-zero exit reports failure.
CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT);
INSERT INTO users(name) VALUES ('alice'), ('bob'), ('carol');
INSERT INTO users(name) VALUES ('dave');
