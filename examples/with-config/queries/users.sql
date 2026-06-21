--! get_user : one
SELECT id, email, name, created_at
FROM users
WHERE id = :id;

--! list_users : many
SELECT id, email, name, created_at
FROM users
ORDER BY id;

--! create_user
INSERT INTO users (email, name, created_at)
VALUES (:email, :name, :created_at);
