-- #48: the channels a codeplug puts in the radio's one user memory group (the
-- Yaesu FTM-300's M-GRP), chosen channel by channel on the codeplug page. Kept
-- per codeplug, like the per-channel scan-list assignment (0009), because the
-- same channel can be in M-GRP in one codeplug and not in another.
CREATE TABLE codeplug_memory_group (
    codeplug_id INTEGER NOT NULL REFERENCES codeplugs(id) ON DELETE CASCADE,
    channel_id  INTEGER NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    PRIMARY KEY (codeplug_id, channel_id)
);
