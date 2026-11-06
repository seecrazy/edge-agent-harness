//! SQLite-backed event and checkpoint storage.
//!
//! The event table is append-only at the database level. Checkpoints are
//! validated snapshots used to bound replay cost; committed events remain the
//! source of truth.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use harness_core::{
    CoreError, Event, EventSink, EventSinkError, EventType, ExecutionSignal, ModelStepCompletion,
    StepCompletion, StepOutcome, Task, TaskStatus,
};
use rusqlite::{params, Connection, OptionalExtension, Row};

/// Errors returned by persistent storage.
#[derive(Debug)]
pub enum StorageError {
    /// SQLite operation failed.
    Sqlite(rusqlite::Error),
    /// A checkpoint or task could not be serialized.
    Serialization(serde_json::Error),
    /// Event was malformed or violated append rules.
    InvalidEvent(&'static str),
    /// Checkpoint was malformed or references unavailable state.
    InvalidCheckpoint(&'static str),
    /// Committed events could not be applied to a restored task.
    Replay(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "sqlite error: {error}"),
            Self::Serialization(error) => write!(f, "serialization error: {error}"),
            Self::InvalidEvent(reason) => write!(f, "invalid event: {reason}"),
            Self::InvalidCheckpoint(reason) => write!(f, "invalid checkpoint: {reason}"),
            Self::Replay(reason) => write!(f, "replay error: {reason}"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Serialization(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value)
    }
}

/// Validated task checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// Task represented by the checkpoint.
    pub task_id: String,
    /// Serialized compact task snapshot.
    pub task_json: String,
    /// Last committed event sequence included by this snapshot.
    pub sequence: u64,
    /// Platform timestamp when the checkpoint was created.
    pub created_at_ms: u64,
    /// Integrity checksum for the snapshot.
    pub checksum: u64,
}

/// Result of transactional event retention maintenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionReport {
    /// Number of tasks whose checkpoint covered earlier events.
    pub compacted_tasks: usize,
    /// Number of events removed.
    pub removed_events: u64,
    /// Number of events remaining after compaction.
    pub retained_events: u64,
}

impl Checkpoint {
    /// Computes a checksum over all persisted checkpoint fields.
    #[must_use]
    pub fn compute_checksum(
        task_id: &str,
        task_json: &str,
        sequence: u64,
        created_at_ms: u64,
    ) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in task_id
            .bytes()
            .chain(task_json.bytes())
            .chain(sequence.to_le_bytes())
            .chain(created_at_ms.to_le_bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        hash
    }
}

/// SQLite-backed event and checkpoint store.
#[derive(Debug)]
pub struct TaskStore {
    connection: Connection,
}

impl TaskStore {
    /// Opens a persistent task store at the supplied path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }

    /// Opens a temporary in-memory store for tests and simulations.
    #[must_use]
    pub fn in_memory() -> Self {
        let connection = Connection::open_in_memory().expect("in-memory sqlite must open");
        Self::initialize(connection).expect("in-memory schema must initialize")
    }

    fn initialize(connection: Connection) -> Result<Self, StorageError> {
        connection.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = FULL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS events (
                sequence INTEGER PRIMARY KEY,
                task_id TEXT NOT NULL,
                event_type INTEGER NOT NULL,
                payload TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                checksum INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS checkpoints (
                task_id TEXT PRIMARY KEY,
                task_json TEXT NOT NULL,
                sequence INTEGER NOT NULL,
                created_at_ms INTEGER NOT NULL,
                checksum INTEGER NOT NULL
            );

            CREATE TRIGGER IF NOT EXISTS events_no_update
            BEFORE UPDATE ON events
            BEGIN
                SELECT RAISE(ABORT, 'events are append-only');
            END;

            CREATE TRIGGER IF NOT EXISTS events_no_delete
            BEFORE DELETE ON events
            BEGIN
                SELECT RAISE(ABORT, 'events are append-only');
            END;
            "#,
        )?;
        Ok(Self { connection })
    }

    /// Appends one validated event in a transaction.
    pub fn append_event(&mut self, event: &Event) -> Result<(), StorageError> {
        let expected_checksum = Event::compute_checksum(
            event.sequence,
            &event.task_id,
            event.event_type,
            &event.payload,
            event.timestamp_ms,
        );
        if event.sequence == 0 || event.checksum != expected_checksum {
            return Err(StorageError::InvalidEvent("sequence or checksum"));
        }

        let transaction = self.connection.transaction()?;
        let (last_sequence, last_timestamp): (Option<u64>, Option<u64>) = transaction
            .query_row(
                "SELECT MAX(sequence), MAX(timestamp_ms) FROM events",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(StorageError::from)?;
        let expected_sequence = last_sequence.unwrap_or_default().saturating_add(1);
        if event.sequence != expected_sequence {
            return Err(StorageError::InvalidEvent("non-monotonic sequence"));
        }
        if let Some(timestamp) = last_timestamp {
            if event.timestamp_ms < timestamp {
                return Err(StorageError::InvalidEvent("non-monotonic timestamp"));
            }
        }

        transaction.execute(
            "INSERT INTO events
                (sequence, task_id, event_type, payload, timestamp_ms, checksum)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                u64_to_i64(event.sequence),
                event.task_id,
                event.event_type.checksum_value(),
                event.payload,
                u64_to_i64(event.timestamp_ms),
                i64::from(event.checksum)
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Loads all committed events in sequence order.
    pub fn load_events(&self) -> Result<Vec<Event>, StorageError> {
        let mut statement = self
            .connection
            .prepare("SELECT sequence, task_id, event_type, payload, timestamp_ms, checksum FROM events ORDER BY sequence")?;
        let rows = statement.query_map([], Self::event_from_row)?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(StorageError::from)?);
        }
        Ok(events)
    }

    /// Loads committed events for one task in sequence order.
    pub fn load_events_for_task(&self, task_id: &str) -> Result<Vec<Event>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT sequence, task_id, event_type, payload, timestamp_ms, checksum
             FROM events
             WHERE task_id = ?1
             ORDER BY sequence",
        )?;
        let rows = statement.query_map([task_id], Self::event_from_row)?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(StorageError::from)?);
        }
        Ok(events)
    }

    fn event_from_row(row: &Row<'_>) -> rusqlite::Result<Event> {
        let sequence: u64 = i64_to_u64(row.get(0)?);
        let task_id: String = row.get(1)?;
        let event_type_value: u8 = row.get(2)?;
        let Some(event_type) = EventType::from_wire_value(event_type_value) else {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Integer,
                Box::new(StorageError::InvalidEvent("unknown event type")),
            ));
        };
        let checksum: i64 = row.get(5)?;
        let checksum = u32::try_from(checksum).map_err(|_| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Integer,
                Box::new(StorageError::InvalidEvent("checksum")),
            )
        })?;
        Ok(Event {
            sequence,
            task_id,
            event_type,
            payload: row.get(3)?,
            timestamp_ms: i64_to_u64(row.get(4)?),
            checksum,
        })
    }

    /// Writes or advances a checkpoint after its event has been committed.
    pub fn save_checkpoint(&mut self, task: &Task, created_at_ms: u64) -> Result<(), StorageError> {
        let transaction = self.connection.transaction()?;
        let sequence: Option<u64> = transaction.query_row(
            "SELECT MAX(sequence) FROM events WHERE task_id = ?1",
            [&task.id],
            |row| row.get(0),
        )?;
        let Some(sequence) = sequence else {
            return Err(StorageError::InvalidCheckpoint(
                "task has no committed event",
            ));
        };
        let task_json = serde_json::to_string(task)?;
        let checksum = Checkpoint::compute_checksum(&task.id, &task_json, sequence, created_at_ms);
        transaction.execute(
            "INSERT INTO checkpoints
                (task_id, task_json, sequence, created_at_ms, checksum)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(task_id) DO UPDATE SET
                task_json = excluded.task_json,
                sequence = excluded.sequence,
                created_at_ms = excluded.created_at_ms,
                checksum = excluded.checksum",
            params![
                task.id,
                task_json,
                u64_to_i64(sequence),
                u64_to_i64(created_at_ms),
                u64_to_i64(checksum)
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Loads a checkpoint for one task.
    pub fn load_checkpoint(&self, task_id: &str) -> Result<Option<Checkpoint>, StorageError> {
        self.connection
            .query_row(
                "SELECT task_id, task_json, sequence, created_at_ms, checksum
                 FROM checkpoints
                 WHERE task_id = ?1",
                [task_id],
                |row| {
                    Ok(Checkpoint {
                        task_id: row.get(0)?,
                        task_json: row.get(1)?,
                        sequence: i64_to_u64(row.get(2)?),
                        created_at_ms: i64_to_u64(row.get(3)?),
                        checksum: i64_to_u64(row.get(4)?),
                    })
                },
            )
            .optional()
            .map_err(StorageError::from)
    }

    /// Restores a task from a validated checkpoint and later committed events.
    pub fn restore_task(&self, task_id: &str) -> Result<Option<Task>, StorageError> {
        let Some(checkpoint) = self.load_checkpoint(task_id)? else {
            return Ok(None);
        };
        let expected_checksum = Checkpoint::compute_checksum(
            &checkpoint.task_id,
            &checkpoint.task_json,
            checkpoint.sequence,
            checkpoint.created_at_ms,
        );
        if checkpoint.checksum != expected_checksum || checkpoint.task_id != task_id {
            return Err(StorageError::InvalidCheckpoint("checksum or task mismatch"));
        }

        let mut task: Task = serde_json::from_str(&checkpoint.task_json)?;
        if task.id != task_id {
            return Err(StorageError::InvalidCheckpoint("task mismatch"));
        }

        let events = self.load_events_for_task(task_id)?;
        if !events
            .iter()
            .any(|event| event.sequence == checkpoint.sequence)
        {
            return Err(StorageError::InvalidCheckpoint(
                "checkpoint event is missing",
            ));
        }

        for event in events
            .iter()
            .filter(|event| event.sequence > checkpoint.sequence)
        {
            let expected = Event::compute_checksum(
                event.sequence,
                &event.task_id,
                event.event_type,
                &event.payload,
                event.timestamp_ms,
            );
            if event.checksum != expected {
                return Err(StorageError::InvalidEvent("checksum"));
            }
            Self::apply_event(&mut task, event)
                .map_err(|error| StorageError::Replay(error.to_string()))?;
        }
        Ok(Some(task))
    }

    /// Returns the latest committed event sequence and timestamp.
    pub fn event_watermark(&self) -> Result<(u64, u64), StorageError> {
        self.connection
            .query_row(
                "SELECT COALESCE(MAX(sequence), 0), COALESCE(MAX(timestamp_ms), 0) FROM events",
                [],
                |row| Ok((i64_to_u64(row.get(0)?), i64_to_u64(row.get(1)?))),
            )
            .map_err(StorageError::from)
    }

    /// Returns the number of committed events.
    pub fn event_count(&self) -> Result<u64, StorageError> {
        self.connection
            .query_row("SELECT COUNT(*) FROM events", [], |row| {
                Ok(i64_to_u64(row.get(0)?))
            })
            .map_err(StorageError::from)
    }

    /// Loads all task checkpoints.
    pub fn load_checkpoints(&self) -> Result<Vec<Checkpoint>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT task_id, task_json, sequence, created_at_ms, checksum
             FROM checkpoints",
        )?;
        let rows = statement.query_map([], checkpoint_from_row)?;
        let mut checkpoints = Vec::new();
        for row in rows {
            checkpoints.push(row.map_err(StorageError::from)?);
        }
        Ok(checkpoints)
    }

    /// Restores all tasks from valid checkpoints and later committed events.
    pub fn restore_all_tasks(&self) -> Result<Vec<Task>, StorageError> {
        let events = self.load_events()?;
        let mut checkpoint_tasks: HashMap<String, Task> = HashMap::new();

        let mut checkpoint_sequences = HashMap::new();
        for checkpoint in self.load_checkpoints()? {
            checkpoint_sequences.insert(checkpoint.task_id.clone(), checkpoint.sequence);
            let task = Self::decode_validated_checkpoint(&self.connection, &checkpoint)?;
            checkpoint_tasks.insert(task.id.clone(), task);
        }

        let mut tasks: HashMap<String, Task> = checkpoint_tasks;
        let mut previous_sequence = 0;

        for event in events {
            if event.sequence <= previous_sequence {
                return Err(StorageError::InvalidEvent("non-monotonic sequence"));
            }
            previous_sequence = event.sequence;
            let expected = Event::compute_checksum(
                event.sequence,
                &event.task_id,
                event.event_type,
                &event.payload,
                event.timestamp_ms,
            );
            if event.checksum != expected {
                return Err(StorageError::InvalidEvent("checksum"));
            }

            if let Some(checkpoint_sequence) = checkpoint_sequences.get(&event.task_id) {
                if event.sequence <= *checkpoint_sequence {
                    continue;
                }
            }

            if event.event_type == EventType::TaskCreated {
                let task: Task = serde_json::from_str(&event.payload)?;
                if task.id != event.task_id || tasks.contains_key(&task.id) {
                    return Err(StorageError::InvalidEvent("task creation mismatch"));
                }
                tasks.insert(task.id.clone(), task);
                continue;
            }

            let Some(task) = tasks.get_mut(&event.task_id) else {
                return Err(StorageError::InvalidEvent("event before task"));
            };
            Self::apply_event(task, &event)
                .map_err(|error| StorageError::Replay(error.to_string()))?;
        }

        Ok(tasks.into_values().collect())
    }

    /// Validates and deserializes a checkpoint without mutating storage.
    fn decode_validated_checkpoint(
        connection: &Connection,
        checkpoint: &Checkpoint,
    ) -> Result<Task, StorageError> {
        let expected_checksum = Checkpoint::compute_checksum(
            &checkpoint.task_id,
            &checkpoint.task_json,
            checkpoint.sequence,
            checkpoint.created_at_ms,
        );
        if checkpoint.checksum != expected_checksum {
            return Err(StorageError::InvalidCheckpoint("checksum mismatch"));
        }

        let task: Task = serde_json::from_str(&checkpoint.task_json)?;
        if task.id != checkpoint.task_id {
            return Err(StorageError::InvalidCheckpoint("task mismatch"));
        }

        let anchor = connection
            .query_row(
                "SELECT sequence, task_id, event_type, payload, timestamp_ms, checksum
                 FROM events
                 WHERE task_id = ?1 AND sequence = ?2",
                params![task.id, u64_to_i64(checkpoint.sequence)],
                Self::event_from_row,
            )
            .map_err(|_| StorageError::InvalidCheckpoint("checkpoint event is missing"))?;
        let expected_anchor_checksum = Event::compute_checksum(
            anchor.sequence,
            &anchor.task_id,
            anchor.event_type,
            &anchor.payload,
            anchor.timestamp_ms,
        );
        if anchor.checksum != expected_anchor_checksum {
            return Err(StorageError::InvalidEvent("checkpoint anchor checksum"));
        }

        Ok(task)
    }

    /// Compacts events already covered by validated checkpoints.
    ///
    /// This is the only storage maintenance path permitted to delete events.
    /// Each checkpoint's anchor event is retained.
    pub fn compact_retention(&mut self) -> Result<CompactionReport, StorageError> {
        let before_count = self.event_count()?;
        let transaction = self.connection.transaction()?;
        let checkpoints = {
            let mut statement = transaction.prepare(
                "SELECT task_id, task_json, sequence, created_at_ms, checksum
                 FROM checkpoints",
            )?;
            let rows = statement.query_map([], checkpoint_from_row)?;
            let mut checkpoints = Vec::new();
            for row in rows {
                checkpoints.push(row.map_err(StorageError::from)?);
            }
            checkpoints
        };

        let mut compacted_tasks = 0;
        let mut removed_events = 0;
        for checkpoint in &checkpoints {
            let expected_checksum = Checkpoint::compute_checksum(
                &checkpoint.task_id,
                &checkpoint.task_json,
                checkpoint.sequence,
                checkpoint.created_at_ms,
            );
            if checkpoint.checksum != expected_checksum {
                return Err(StorageError::InvalidCheckpoint("checksum mismatch"));
            }
            Self::decode_validated_checkpoint(&transaction, checkpoint)?;
        }

        transaction.execute_batch("DROP TRIGGER IF EXISTS events_no_delete;")?;
        for checkpoint in &checkpoints {
            let removed: usize = transaction.execute(
                "DELETE FROM events WHERE task_id = ?1 AND sequence < ?2",
                params![checkpoint.task_id, u64_to_i64(checkpoint.sequence)],
            )?;
            removed_events += u64::try_from(removed)
                .map_err(|_| StorageError::InvalidEvent("deleted event count overflow"))?;
            compacted_tasks += 1;
        }
        transaction.execute_batch(
            r#"
            CREATE TRIGGER IF NOT EXISTS events_no_delete
            BEFORE DELETE ON events
            BEGIN
                SELECT RAISE(ABORT, 'events are append-only');
            END;
            "#,
        )?;
        transaction.commit()?;

        let after_count = self.event_count()?;
        if after_count + removed_events != before_count {
            return Err(StorageError::InvalidEvent("compaction count mismatch"));
        }

        Ok(CompactionReport {
            compacted_tasks,
            removed_events,
            retained_events: after_count,
        })
    }

    /// Compacts only when the committed event count exceeds a threshold.
    pub fn compact_retention_if_over(
        &mut self,
        max_events: u64,
    ) -> Result<CompactionReport, StorageError> {
        let current = self.event_count()?;
        if current <= max_events {
            return Ok(CompactionReport {
                compacted_tasks: 0,
                removed_events: 0,
                retained_events: current,
            });
        }
        self.compact_retention()
    }

    fn apply_event(task: &mut Task, event: &Event) -> Result<(), CoreError> {
        match event.event_type {
            EventType::TaskCreated => Ok(()),
            EventType::StateChanged => {
                let Some(status) = TaskStatus::parse_wire_name(&event.payload) else {
                    return Err(CoreError::EventLog(
                        harness_core::EventLogError::ChecksumMismatch,
                    ));
                };
                task.transition(status, event.timestamp_ms)
            }
            EventType::ModelStepConsumed => {
                let Ok(tokens) = event.payload.parse::<u64>() else {
                    return Err(CoreError::EventLog(
                        harness_core::EventLogError::ChecksumMismatch,
                    ));
                };
                task.consume_model_step(tokens, event.timestamp_ms)
            }
            EventType::ToolCallConsumed => task.consume_tool_call(event.timestamp_ms),
            EventType::TaskFinished => {
                let Some(status) = TaskStatus::parse_wire_name(&event.payload) else {
                    return Err(CoreError::EventLog(
                        harness_core::EventLogError::ChecksumMismatch,
                    ));
                };
                task.transition(status, event.timestamp_ms)
            }
            EventType::TaskCancelled => task.transition(TaskStatus::Cancelled, event.timestamp_ms),
            EventType::TaskFailed => task.transition(TaskStatus::Failed, event.timestamp_ms),
        }
    }
}

/// A write-ahead adapter around a shared SQLite task store.
#[derive(Debug)]
struct TaskStoreSink {
    store: Arc<Mutex<TaskStore>>,
}

impl EventSink for TaskStoreSink {
    fn append_event(&mut self, event: &Event) -> Result<(), EventSinkError> {
        let mut store = self
            .store
            .lock()
            .map_err(|error| EventSinkError::External(error.to_string()))?;
        store
            .append_event(event)
            .map_err(|error| EventSinkError::External(error.to_string()))
    }
}

/// Supervisor that automatically commits events to SQLite and restores on open.
#[derive(Debug)]
pub struct PersistentTaskSupervisor {
    supervisor: harness_core::TaskSupervisor,
    store: Arc<Mutex<TaskStore>>,
}

impl PersistentTaskSupervisor {
    /// Opens or creates a persistent supervisor database.
    pub fn open(
        path: impl AsRef<Path>,
        admission: harness_core::AdmissionController,
        event_capacity: usize,
    ) -> Result<Self, StorageError> {
        let store = Arc::new(Mutex::new(TaskStore::open(path)?));
        let (last_sequence, last_timestamp_ms) = {
            let store = store
                .lock()
                .map_err(|error| StorageError::Replay(error.to_string()))?;
            store.event_watermark()?
        };

        let mut supervisor = harness_core::TaskSupervisor::with_event_sink(
            admission,
            event_capacity,
            Box::new(TaskStoreSink {
                store: Arc::clone(&store),
            }),
        );
        let restored = {
            let store = store
                .lock()
                .map_err(|error| StorageError::Replay(error.to_string()))?;
            store.restore_all_tasks()?
        };
        supervisor
            .restore_tasks(restored)
            .map_err(|error| StorageError::Replay(error.to_string()))?;
        supervisor.sync_event_watermark(last_sequence, last_timestamp_ms);

        Ok(Self { supervisor, store })
    }

    /// Creates a task after admission and durable event commit.
    pub fn create_task(
        &mut self,
        request: harness_core::CreateTaskRequest<'_>,
    ) -> Result<harness_core::Task, StorageError> {
        self.supervisor
            .create_task(request)
            .map_err(|error| StorageError::Replay(error.to_string()))
            .cloned()
    }

    /// Applies a state transition after durable event commit.
    pub fn transition(
        &mut self,
        task_id: &str,
        next: TaskStatus,
        now_ms: u64,
    ) -> Result<harness_core::Task, StorageError> {
        self.supervisor
            .transition(task_id, next, now_ms)
            .map_err(|error| StorageError::Replay(error.to_string()))
            .cloned()
    }

    /// Pauses a task durably.
    pub fn pause_task(
        &mut self,
        task_id: &str,
        now_ms: u64,
    ) -> Result<harness_core::Task, StorageError> {
        self.supervisor
            .pause_task(task_id, now_ms)
            .map_err(|error| StorageError::Replay(error.to_string()))
            .cloned()
    }

    /// Resumes a paused task durably.
    pub fn resume_task(
        &mut self,
        task_id: &str,
        now_ms: u64,
    ) -> Result<harness_core::Task, StorageError> {
        self.supervisor
            .resume_task(task_id, now_ms)
            .map_err(|error| StorageError::Replay(error.to_string()))
            .cloned()
    }

    /// Records model token usage after durable event commit.
    pub fn consume_model_step(
        &mut self,
        task_id: &str,
        tokens: u64,
        now_ms: u64,
    ) -> Result<harness_core::Task, StorageError> {
        self.supervisor
            .consume_model_step(task_id, tokens, now_ms)
            .map_err(|error| StorageError::Replay(error.to_string()))
            .cloned()
    }

    /// Records a tool call after durable event commit.
    pub fn consume_tool_call(
        &mut self,
        task_id: &str,
        now_ms: u64,
    ) -> Result<harness_core::Task, StorageError> {
        self.supervisor
            .consume_tool_call(task_id, now_ms)
            .map_err(|error| StorageError::Replay(error.to_string()))
            .cloned()
    }

    /// Executes one durable, bounded model step through the inner supervisor.
    pub fn execute_model_step<F>(
        &mut self,
        task_id: &str,
        signal: &ExecutionSignal,
        started_at_ms: u64,
        operation: F,
    ) -> Result<StepOutcome<u64>, StorageError>
    where
        F: FnOnce(&ExecutionSignal) -> Result<ModelStepCompletion, CoreError>,
    {
        self.supervisor
            .execute_model_step(task_id, signal, started_at_ms, operation)
            .map_err(|error| StorageError::Replay(error.to_string()))
    }

    /// Executes one durable, bounded tool step through the inner supervisor.
    pub fn execute_tool_step<F>(
        &mut self,
        task_id: &str,
        signal: &ExecutionSignal,
        started_at_ms: u64,
        operation: F,
    ) -> Result<StepOutcome<()>, StorageError>
    where
        F: FnOnce(&ExecutionSignal) -> Result<StepCompletion, CoreError>,
    {
        self.supervisor
            .execute_tool_step(task_id, signal, started_at_ms, operation)
            .map_err(|error| StorageError::Replay(error.to_string()))
    }

    /// Durably cancels a task.
    pub fn cancel_task(
        &mut self,
        task_id: &str,
        reason: &str,
        now_ms: u64,
    ) -> Result<harness_core::Task, StorageError> {
        self.supervisor
            .cancel_task(task_id, reason, now_ms)
            .map_err(|error| StorageError::Replay(error.to_string()))
            .cloned()
    }

    /// Durably fails a task.
    pub fn fail_task(
        &mut self,
        task_id: &str,
        reason: &str,
        now_ms: u64,
    ) -> Result<harness_core::Task, StorageError> {
        self.supervisor
            .fail_task(task_id, reason, now_ms)
            .map_err(|error| StorageError::Replay(error.to_string()))
            .cloned()
    }

    /// Returns a restored task.
    #[must_use]
    pub fn task(&self, task_id: &str) -> Option<harness_core::Task> {
        self.supervisor.task(task_id).cloned()
    }

    /// Returns the bounded recent-event cache.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        self.supervisor.events().to_vec()
    }

    /// Saves a checkpoint for a task.
    pub fn save_checkpoint(&mut self, task_id: &str, now_ms: u64) -> Result<(), StorageError> {
        let task = self
            .supervisor
            .task(task_id)
            .cloned()
            .ok_or(StorageError::InvalidCheckpoint("task not found"))?;
        let mut store = self
            .store
            .lock()
            .map_err(|error| StorageError::Replay(error.to_string()))?;
        store.save_checkpoint(&task, now_ms)
    }

    /// Checkpoints all known tasks and compacts events covered by checkpoints.
    ///
    /// This operation is maintenance-only. It does not expose event deletion to
    /// model or tool code.
    pub fn compact_retention(&mut self, now_ms: u64) -> Result<CompactionReport, StorageError> {
        let tasks = self.supervisor.tasks();
        let mut store = self
            .store
            .lock()
            .map_err(|error| StorageError::Replay(error.to_string()))?;
        for task in &tasks {
            store.save_checkpoint(task, now_ms)?;
        }
        store.compact_retention()
    }

    /// Compacts only when the committed event count exceeds a threshold.
    pub fn compact_if_over(
        &mut self,
        max_events: u64,
        now_ms: u64,
    ) -> Result<CompactionReport, StorageError> {
        let mut store = self
            .store
            .lock()
            .map_err(|error| StorageError::Replay(error.to_string()))?;
        let tasks = self.supervisor.tasks();
        for task in &tasks {
            store.save_checkpoint(task, now_ms)?;
        }
        store.compact_retention_if_over(max_events)
    }
}

fn u64_to_i64(value: u64) -> i64 {
    i64::from_ne_bytes(value.to_ne_bytes())
}

fn i64_to_u64(value: i64) -> u64 {
    u64::from_ne_bytes(value.to_ne_bytes())
}

fn checkpoint_from_row(row: &Row<'_>) -> rusqlite::Result<Checkpoint> {
    Ok(Checkpoint {
        task_id: row.get(0)?,
        task_json: row.get(1)?,
        sequence: i64_to_u64(row.get(2)?),
        created_at_ms: i64_to_u64(row.get(3)?),
        checksum: i64_to_u64(row.get(4)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_core::{TaskBudget, TaskStatus};
    use harness_policy::TaskClass;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should advance")
            .as_nanos();
        std::env::temp_dir().join(format!("edge-agent-{name}-{unique}.sqlite"))
    }

    fn valid_event(sequence: u64) -> Event {
        let task_id = "task-1";
        let payload = "test";
        Event {
            sequence,
            task_id: task_id.to_owned(),
            event_type: EventType::TaskCreated,
            payload: payload.to_owned(),
            timestamp_ms: sequence,
            checksum: Event::compute_checksum(
                sequence,
                task_id,
                EventType::TaskCreated,
                payload,
                sequence,
            ),
        }
    }

    #[test]
    fn rejects_invalid_event_checksum() {
        let mut store = TaskStore::in_memory();
        let mut event = valid_event(1);
        event.checksum = event.checksum.wrapping_add(1);

        assert!(matches!(
            store.append_event(&event),
            Err(StorageError::InvalidEvent(_))
        ));
        assert!(store.load_events().unwrap().is_empty());
    }

    #[test]
    fn event_table_is_append_only() {
        let mut store = TaskStore::in_memory();
        store.append_event(&valid_event(1)).unwrap();

        let result = store.connection.execute(
            "UPDATE events SET payload = 'tampered' WHERE sequence = 1",
            [],
        );
        assert!(result.is_err());
        assert_eq!(store.load_events().unwrap()[0].payload, "test");
    }

    #[test]
    fn checkpoint_and_replay_survive_reopen() {
        let path = temp_path("replay");
        let expected = Task {
            id: "task-1".into(),
            session_id: "session-1".into(),
            goal: "Read status".into(),
            task_class: TaskClass::FastQa,
            status: TaskStatus::Pending,
            budget: TaskBudget::fast_qa(),
            usage: Default::default(),
            created_at: 1_000,
            updated_at: 1_000,
        };
        {
            let mut store = TaskStore::open(&path).unwrap();
            let task = expected.clone();
            store.append_event(&valid_event(1)).unwrap();
            store.save_checkpoint(&task, 1_000).unwrap();
        }

        let reopened = TaskStore::open(&path).unwrap();
        let restored = reopened.restore_task("task-1").unwrap().unwrap();
        assert_eq!(restored, expected);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn replay_restores_state_and_usage() {
        let mut store = TaskStore::in_memory();
        let mut task = Task {
            id: "task-1".into(),
            session_id: "session-1".into(),
            goal: "Read status".into(),
            task_class: TaskClass::FastQa,
            status: TaskStatus::Pending,
            budget: TaskBudget::fast_qa(),
            usage: Default::default(),
            created_at: 1_000,
            updated_at: 1_000,
        };
        store.append_event(&valid_event(1)).unwrap();
        store.save_checkpoint(&task, 1_000).unwrap();

        task.transition(TaskStatus::Planning, 1_001).unwrap();
        let state_event = Event {
            sequence: 2,
            task_id: task.id.clone(),
            event_type: EventType::StateChanged,
            payload: TaskStatus::Planning.wire_name().to_owned(),
            timestamp_ms: 1_001,
            checksum: Event::compute_checksum(
                2,
                &task.id,
                EventType::StateChanged,
                TaskStatus::Planning.wire_name(),
                1_001,
            ),
        };
        store.append_event(&state_event).unwrap();

        task.transition(TaskStatus::Running, 1_002).unwrap();
        let running_event = Event {
            sequence: 3,
            task_id: task.id.clone(),
            event_type: EventType::StateChanged,
            payload: TaskStatus::Running.wire_name().to_owned(),
            timestamp_ms: 1_002,
            checksum: Event::compute_checksum(
                3,
                &task.id,
                EventType::StateChanged,
                TaskStatus::Running.wire_name(),
                1_002,
            ),
        };
        store.append_event(&running_event).unwrap();

        task.consume_model_step(32, 1_003).unwrap();
        let model_event = Event {
            sequence: 4,
            task_id: task.id.clone(),
            event_type: EventType::ModelStepConsumed,
            payload: "32".into(),
            timestamp_ms: 1_003,
            checksum: Event::compute_checksum(
                4,
                &task.id,
                EventType::ModelStepConsumed,
                "32",
                1_003,
            ),
        };
        store.append_event(&model_event).unwrap();

        let restored = store.restore_task("task-1").unwrap().unwrap();
        assert_eq!(restored.status, TaskStatus::Running);
        assert_eq!(restored.usage.model_tokens, 32);
        assert_eq!(restored.usage.elapsed_ms, 3);
    }

    #[test]
    fn compaction_retains_checkpoint_anchor_and_restores_state() {
        let mut store = TaskStore::in_memory();
        let mut task = Task {
            id: "task-compact".into(),
            session_id: "session-1".into(),
            goal: "Read status".into(),
            task_class: TaskClass::FastQa,
            status: TaskStatus::Pending,
            budget: TaskBudget::fast_qa(),
            usage: Default::default(),
            created_at: 1_000,
            updated_at: 1_000,
        };
        let created_event = Event {
            sequence: 1,
            task_id: task.id.clone(),
            event_type: EventType::TaskCreated,
            payload: serde_json::to_string(&task).unwrap(),
            timestamp_ms: 1_000,
            checksum: Event::compute_checksum(
                1,
                &task.id,
                EventType::TaskCreated,
                &serde_json::to_string(&task).unwrap(),
                1_000,
            ),
        };
        store.append_event(&created_event).unwrap();

        task.transition(TaskStatus::Planning, 1_001).unwrap();
        let state_event = Event {
            sequence: 2,
            task_id: task.id.clone(),
            event_type: EventType::StateChanged,
            payload: TaskStatus::Planning.wire_name().to_owned(),
            timestamp_ms: 1_001,
            checksum: Event::compute_checksum(
                2,
                &task.id,
                EventType::StateChanged,
                TaskStatus::Planning.wire_name(),
                1_001,
            ),
        };
        store.append_event(&state_event).unwrap();

        task.transition(TaskStatus::Running, 1_002).unwrap();
        task.consume_model_step(48, 1_003).unwrap();
        let model_event = Event {
            sequence: 3,
            task_id: task.id.clone(),
            event_type: EventType::ModelStepConsumed,
            payload: "48".into(),
            timestamp_ms: 1_003,
            checksum: Event::compute_checksum(
                3,
                &task.id,
                EventType::ModelStepConsumed,
                "48",
                1_003,
            ),
        };
        store.append_event(&model_event).unwrap();
        store.save_checkpoint(&task, 1_003).unwrap();

        let report = store.compact_retention().unwrap();

        assert_eq!(report.compacted_tasks, 1);
        assert_eq!(report.removed_events, 2);
        assert_eq!(report.retained_events, 1);
        assert_eq!(store.load_events_for_task("task-compact").unwrap().len(), 1);

        let restored = store.restore_task("task-compact").unwrap().unwrap();
        assert_eq!(restored.status, TaskStatus::Running);
        assert_eq!(restored.usage.model_tokens, 48);
        assert_eq!(restored.usage.elapsed_ms, 3);

        assert!(store
            .connection
            .execute("DELETE FROM events WHERE sequence = 3", [])
            .is_err());
    }

    #[test]
    fn compact_if_over_only_compacts_above_threshold() {
        let mut store = TaskStore::in_memory();
        let task = Task {
            id: "task-1".into(),
            session_id: "session-1".into(),
            goal: "threshold".into(),
            task_class: TaskClass::FastQa,
            status: TaskStatus::Pending,
            budget: TaskBudget::fast_qa(),
            usage: Default::default(),
            created_at: 1_000,
            updated_at: 1_000,
        };
        for sequence in 1..=3 {
            store.append_event(&valid_event(sequence)).unwrap();
        }
        store.save_checkpoint(&task, 1_000).unwrap();

        let noop = store.compact_retention_if_over(100).unwrap();
        assert_eq!(noop.compacted_tasks, 0);
        assert_eq!(noop.removed_events, 0);
        assert_eq!(noop.retained_events, 3);

        let report = store.compact_retention_if_over(2).unwrap();
        assert_eq!(report.compacted_tasks, 1);
        assert_eq!(report.removed_events, 2);
        assert_eq!(report.retained_events, 1);
    }
}
