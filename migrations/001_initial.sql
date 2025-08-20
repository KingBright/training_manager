-- migrations/001_initial.sql
-- 创建任务表
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    command TEXT NOT NULL,
    conda_env TEXT,  -- 新增conda环境字段
    status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'failed', 'stopped')),
    created_at DATETIME NOT NULL,
    started_at DATETIME,
    finished_at DATETIME,
    log_path TEXT,
    tensorboard_port INTEGER
);

-- 创建同步记录表
CREATE TABLE sync_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_path TEXT NOT NULL,
    target_path TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('success', 'failed')),
    sync_time DATETIME NOT NULL,
    output TEXT
);

-- 创建配置表
CREATE TABLE config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at DATETIME NOT NULL
);

-- 插入默认配置
INSERT INTO config (key, value, updated_at) VALUES 
('output_path', './outputs', datetime('now')),
('sync_target_path', '/opt/isaaclab/source', datetime('now'));

-- 创建索引
CREATE INDEX idx_tasks_status ON tasks(status);
CREATE INDEX idx_tasks_created_at ON tasks(created_at);
CREATE INDEX idx_sync_records_sync_time ON sync_records(sync_time);