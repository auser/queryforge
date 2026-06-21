--! get_user : one
--: id: crate::UserId
--: column.email: crate::EmailAddress
SELECT id, email
FROM users
WHERE id = :id;

--! list_users : many
SELECT id, email
FROM users
ORDER BY id;
