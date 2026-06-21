--! create_user
INSERT INTO users (id, email, name, active)
VALUES (:id, :email, :name, :active);

--! get_user : optional
SELECT id, email, name, active
FROM users
WHERE id = :id;

--! list_users
SELECT id, email, name, active
FROM users
ORDER BY id;

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
