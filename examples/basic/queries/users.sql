--! get_user : one
SELECT id, email, created_at
FROM users
WHERE id = :id;

--! list_users : many
SELECT id, email, created_at
FROM users
ORDER BY created_at DESC;
