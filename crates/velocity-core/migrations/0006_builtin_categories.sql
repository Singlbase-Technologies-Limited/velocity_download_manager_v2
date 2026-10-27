-- Schema 6 (#31): the built-in categories, as in V1. They can be edited but not deleted, and a
-- download's automatic category is the first, by `sort_order`, that lists its extension. Before
-- this schema there was no way to add a category, so the table is empty and no name collides.
INSERT INTO categories (id, name, color, icon, extensions_json, builtin, sort_order) VALUES
 ('compressed', 'Compressed',  '#a855f7', 'archive',    '["zip","rar","7z","tar","gz","tgz","bz2","xz","zst","cab","arj","lzh"]', 1, 1),
 ('documents',  'Documents',   '#0ea5e9', 'file-text',  '["pdf","doc","docx","xls","xlsx","ppt","pptx","odt","ods","odp","txt","rtf","epub","csv"]', 1, 2),
 ('music',      'Music',       '#ec4899', 'music',      '["mp3","flac","wav","aac","ogg","opus","m4a","wma","aiff"]', 1, 3),
 ('video',      'Video',       '#f97316', 'film',       '["mp4","mkv","avi","mov","wmv","webm","m4v","flv","mpg","mpeg","ts"]', 1, 4),
 ('programs',   'Programs',    '#22c55e', 'app-window', '["exe","msi","msix","appx","dmg","pkg","deb","rpm","appimage","apk","flatpak","snap"]', 1, 5),
 ('images',     'Disk images', '#64748b', 'disc',       '["iso","img","vhd","vhdx","vmdk","qcow2"]', 1, 6);
