CREATE TABLE IF NOT EXISTS projects (
    id INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    repo_id TEXT NOT NULL,
    created INTEGER NOT NULL,
    env TEXT NOT NULL,
    root TEXT NOT NULL,
    prod_id INTENGER -- deployment id used for prod
);

CREATE TABLE IF NOT EXISTS deployments (
    id INTEGER PRIMARY KEY NOT NULL,
    url_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL, -- this is the commit timestamp, used for sorting
    created INTEGER NOT NULL,
    env TEXT NOT NULL,
    sha TEXT NOT NULL,
    -- branch TEXT NOT NULL,
    default_branch INTEGER NOT NULL, -- 0 false 1 true
    build_started INTEGER,
    build_finished INTEGER,
    result TEXT,
    project INTEGER NOT NULL,
    FOREIGN KEY (project) REFERENCES projects(id) ON DELETE CASCADE,
    UNIQUE(project, url_id)
);

CREATE TABLE IF NOT EXISTS build (
    id INTEGER PRIMARY KEY NOT NULL,
    timestamp INTEGER NOT NULL,
    content TEXT NOT NULL,
    error INTEGER NOT NULL,
    deployment INTEGER NOT NULL,
    FOREIGN KEY (deployment) REFERENCES deployments(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS domains (
    domain TEXT PRIMARY KEY NOT NULL,
    project INTEGER NOT NULL,
    FOREIGN KEY (project) REFERENCES projects(id) ON DELETE CASCADE
);
