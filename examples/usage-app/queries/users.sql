--! get_user : one
SELECT id, email
FROM users
WHERE id = :id;

--! list_users : many
SELECT id, email
FROM users
ORDER BY id;
