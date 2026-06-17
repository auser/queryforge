# QueryForge CRUD Example

This example shows SQL-first CRUD-style mutations without `: exec` annotations.

QueryForge does not auto-generate CRUD from a table. You still write the SQL you want, and QueryForge infers normal mutation blocks as execution queries:

```sql
--! create_user
INSERT INTO users (id, email, name, active)
VALUES (:id, :email, :name, :active);

--! update_user
UPDATE users
SET email = :email, name = :name, active = :active
WHERE id = :id;

--! upsert_user
INSERT INTO users (id, email, name, active)
VALUES (:id, :email, :name, :active)
ON CONFLICT(id) DO UPDATE
SET email = excluded.email,
    name = excluded.name,
    active = excluded.active;

--! delete_user
DELETE FROM users
WHERE id = :id;
```

Run it from the workspace root:

```bash
cargo run -p queryforge-crud-example
cargo test -p queryforge-crud-example
```

The binary and test both create an in-memory libSQL database and exercise generated create, read, update, upsert, list, and delete functions.
