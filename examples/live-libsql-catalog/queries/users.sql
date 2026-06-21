--! get_user : one
SELECT
    u.id,
    u.email,
    u.display_name,
    u.active,
    o.slug AS org_slug
FROM users u
JOIN organizations o ON o.id = u.org_id
WHERE u.id = :id;

--! list_users : many
SELECT
    u.id,
    u.email,
    u.display_name,
    u.active
FROM users u
ORDER BY u.id;
