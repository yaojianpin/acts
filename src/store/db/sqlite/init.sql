CREATE TABLE IF NOT EXISTS act_model (
    'id' VARCHAR(32) PRIMARY KEY NOT NULL,
    'model' TEXT NOT NULL,
    'ver' INT NOT NULL
);
CREATE TABLE IF NOT EXISTS act_proc (
    'id' VARCHAR(32) PRIMARY KEY NOT NULL,
    'pid' VARCHAR(24) NOT NULL,
    'model' TEXT NOT NULL,
    'state' VARCHAR(200) NOT NULL,
    'vars' TEXT NOT NULL,
    'start_time' BIGINT,
    'end_time' BIGINT
);
CREATE TABLE IF NOT EXISTS act_task (
    'id' VARCHAR(32) PRIMARY KEY NOT NULL,
    'tag' VARCHAR(10) NOT NULL,
    'pid' VARCHAR(24) NOT NULL,
    'tid' VARCHAR(8) NOT NULL,
    'nid' VARCHAR(8) NOT NULL,
    'state' VARCHAR(200) NOT NULL,
    'user' VARCHAR(32) NOT NULL,
    'start_time' BIGINT,
    'end_time' BIGINT
);
CREATE TABLE IF NOT EXISTS act_act (
    'id' VARCHAR(32) PRIMARY KEY NOT NULL,
    'kind' VARCHAR(32) NOT NULL,
    'event' VARCHAR(32) NOT NULL,
    'pid' VARCHAR(24) NOT NULL,
    'tid' VARCHAR(32) NOT NULL,
    'vars' VARCHAR(2000) NOT NULL,
    'create_time' BIGINT,
    'update_time' BIGINT,
    'state' INT,
    'ack' BIT,
    'active' BIT
);