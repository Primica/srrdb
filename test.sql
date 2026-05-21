-- USERS
insert into USERS (USERNAME, EMAIL, PSSWRD) values
('admin', 'admin@test.com', '$2y$10$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX'),
('Alice', 'alice@test.com', '$2y$10$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX'),
('Bob', 'bob@test.com', '$2y$10$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX'),
('Charlie', 'charlie@test.com', '$2y$10$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX'),
('Diana', 'diana@test.com', '$2y$10$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX'),
('Eve', 'eve@test.com', '$2y$10$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX');

-- POSTS (longer 2–3 line content)
insert into POSTS (TITLE, CONTENT) values
('Learning SQL Basics',
'Today I finally understood how SQL works with real databases.
Creating tables and inserting data feels very powerful.
I am excited to use this knowledge in real projects.'),

('Why I Love Programming',
'Programming lets me turn ideas into real things.
Even small scripts feel rewarding once they work.
Debugging can be painful, but the result is worth it.'),

('A Rainy Afternoon',
'It rained all day today and the streets were completely empty.
I stayed inside, listened to music, and worked on personal projects.
Sometimes quiet days are the most productive.'),

('First Blog Project',
'I decided to build a simple blog system using MySQL.
Designing the database schema was harder than expected.
Still, seeing everything connect is satisfying.'),

('Late Night Coding',
'I stayed up way too late fixing a small bug.
It turned out to be a missing semicolon.
Classic mistake, but lesson learned.'),

('Thoughts on Databases',
'Databases are more important than I thought.
Good structure makes everything easier later.
Bad design becomes a nightmare very quickly.'),

('Weekend Plans',
'This weekend I want to clean my codebase.
I also plan to add comments and documentation.
Future me will definitely appreciate that.'),

('Learning From Mistakes',
'Breaking things is part of learning programming.
Every error message teaches something new.
The key is to not give up.'),

('Simple Is Better',
'I tried to over-engineer a small project.
In the end, a simple solution worked best.
Complexity is not always intelligence.'),

('Building Habits',
'Coding a little every day builds strong habits.
Even 30 minutes makes a difference.
Consistency beats motivation.');

-- COMMENTS
insert into COMMENTS (CONTENT, USER_ID, POST_ID) values
('SQL clicks after practice, you are on the right path.', 2, 1),
('Databases felt scary to me at first too.', 3, 1),
('This is exactly why I started coding.', 4, 2),
('Debugging pain is universal.', 5, 2),
('Rainy days are perfect for coding.', 6, 3),
('That sounds peaceful honestly.', 1, 3),
('Schema design is the hardest part.', 2, 4),
('Once it works, it feels amazing.', 3, 4),
('Missing semicolons are evil.', 4, 5),
('I lost hours to that exact mistake.', 5, 5),
('Database design saves so much time later.', 6, 6),
('Bad schemas haunt you forever.', 1, 6),
('Cleaning code feels refreshing.', 2, 7),
('Documentation is always worth it.', 3, 7),
('Errors are the best teachers.', 4, 8),
('Giving up is the only real failure.', 5, 8),
('Over-engineering is a real trap.', 6, 9),
('Simple solutions scale better too.', 1, 9),
('Daily habits beat long sessions.', 2, 10),
('Consistency really compounds.', 3, 10);
