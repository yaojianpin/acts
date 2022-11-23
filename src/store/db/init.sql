CREATE TABLE
  IF NOT EXISTS act_proc (
    'id' VARCHAR(32) PRIMARY KEY NOT NULL,
    'pid' VARCHAR(24) NOT NULL,
    'model' TEXT NOT NULL,
    'state' VARCHAR(200) NOT NULL,
    'vars' TEXT NOT NULL
  );

CREATE TABLE
  IF NOT EXISTS act_task (
    'id' VARCHAR(32) PRIMARY KEY NOT NULL,
    'tag' VARCHAR(10) NOT NULL,
    'pid' VARCHAR(24) NOT NULL,
    'tid' VARCHAR(8) NOT NULL,
    'state' VARCHAR(200) NOT NULL,
    'user' VARCHAR(32) NOT NULL,
    'start_time' INTEGER,
    'end_time' INTEGER
  );

CREATE TABLE
  IF NOT EXISTS act_message (
    'id' VARCHAR(32) PRIMARY KEY NOT NULL,
    'pid' VARCHAR(24) NOT NULL,
    'tid' VARCHAR(8) NOT NULL,
    'user' VARCHAR(200) NOT NULL,
    'create_time' INTEGER
  );