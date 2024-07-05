use std::fmt;
use std::ops::Bound;

use anyerror::AnyError;

use crate::storage::SnapshotSignature;
use crate::LogId;
use crate::RaftTypeConfig;
use crate::Vote;

/// Convert error to StorageError::IO();
pub trait ToStorageResult<C, T>
where C: RaftTypeConfig
{
    /// Convert Result<T, E> to Result<T, StorageError::IO(StorageIOError)>
    ///
    /// `f` provides error context for building the StorageIOError.
    fn sto_res<F>(self, f: F) -> Result<T, StorageError<C>>
    where F: FnOnce() -> (ErrorSubject<C>, ErrorVerb);
}

impl<C, T> ToStorageResult<C, T> for Result<T, std::io::Error>
where C: RaftTypeConfig
{
    fn sto_res<F>(self, f: F) -> Result<T, StorageError<C>>
    where F: FnOnce() -> (ErrorSubject<C>, ErrorVerb) {
        match self {
            Ok(x) => Ok(x),
            Err(e) => {
                let (subject, verb) = f();
                let io_err = StorageIOError::new(subject, verb, AnyError::new(&e));
                Err(io_err.into())
            }
        }
    }
}

/// An error that occurs when the RaftStore impl runs defensive check of input or output.
/// E.g. re-applying an log entry is a violation that may be a potential bug.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize), serde(bound = ""))]
pub struct DefensiveError<C>
where C: RaftTypeConfig
{
    /// The subject that violates store defensive check, e.g. hard-state, log or state machine.
    pub subject: ErrorSubject<C>,

    /// The description of the violation.
    pub violation: Violation<C>,

    pub backtrace: Option<String>,
}

impl<C> DefensiveError<C>
where C: RaftTypeConfig
{
    pub fn new(subject: ErrorSubject<C>, violation: Violation<C>) -> Self {
        Self {
            subject,
            violation,
            backtrace: anyerror::backtrace_str(),
        }
    }
}

impl<C> fmt::Display for DefensiveError<C>
where C: RaftTypeConfig
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{:?}' violates: '{}'", self.subject, self.violation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize), serde(bound = ""))]
pub enum ErrorSubject<C>
where C: RaftTypeConfig
{
    /// A general storage error
    Store,

    /// HardState related error.
    Vote,

    /// Error that is happened when operating a series of log entries
    Logs,

    /// Error about a single log entry
    Log(LogId<C::NodeId>),

    /// Error about a single log entry without knowing the log term.
    LogIndex(u64),

    /// Error happened when applying a log entry
    Apply(LogId<C::NodeId>),

    /// Error happened when operating state machine.
    StateMachine,

    /// Error happened when operating snapshot.
    Snapshot(Option<SnapshotSignature<C>>),

    None,
}

/// What it is doing when an error occurs.
#[derive(Debug)]
#[derive(Clone, Copy)]
#[derive(PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum ErrorVerb {
    Read,
    Write,
    Seek,
    Delete,
}

impl fmt::Display for ErrorVerb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Violations a store would return when running defensive check.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize), serde(bound = ""))]
pub enum Violation<C>
where C: RaftTypeConfig
{
    #[error("term can only be change to a greater value, current: {curr}, change to {to}")]
    VoteNotAscending { curr: Vote<C::NodeId>, to: Vote<C::NodeId> },

    #[error("voted_for can not change from Some() to other Some(), current: {curr:?}, change to {to:?}")]
    NonIncrementalVote { curr: Vote<C::NodeId>, to: Vote<C::NodeId> },

    #[error("log at higher index is obsolete: {higher_index_log_id:?} should GT {lower_index_log_id:?}")]
    DirtyLog {
        higher_index_log_id: LogId<C::NodeId>,
        lower_index_log_id: LogId<C::NodeId>,
    },

    #[error("try to get log at index {want} but got {got:?}")]
    LogIndexNotFound { want: u64, got: Option<u64> },

    #[error("range is empty: start: {start:?}, end: {end:?}")]
    RangeEmpty { start: Option<u64>, end: Option<u64> },

    #[error("range is not half-open: start: {start:?}, end: {end:?}")]
    RangeNotHalfOpen { start: Bound<u64>, end: Bound<u64> },

    // TODO(xp): rename this to some input related error name.
    #[error("empty log vector")]
    LogsEmpty,

    #[error("all logs are removed. It requires at least one log to track continuity")]
    StoreLogsEmpty,

    #[error("logs are not consecutive, prev: {prev:?}, next: {next}")]
    LogsNonConsecutive {
        prev: Option<LogId<C::NodeId>>,
        next: LogId<C::NodeId>,
    },

    #[error("invalid next log to apply: prev: {prev:?}, next: {next}")]
    ApplyNonConsecutive {
        prev: Option<LogId<C::NodeId>>,
        next: LogId<C::NodeId>,
    },

    #[error("applied log can not conflict, last_applied: {last_applied:?}, delete since: {first_conflict_log_id}")]
    AppliedWontConflict {
        last_applied: Option<LogId<C::NodeId>>,
        first_conflict_log_id: LogId<C::NodeId>,
    },

    #[error("not allowed to purge non-applied logs, last_applied: {last_applied:?}, purge upto: {purge_upto}")]
    PurgeNonApplied {
        last_applied: Option<LogId<C::NodeId>>,
        purge_upto: LogId<C::NodeId>,
    },
}

/// A storage error could be either a defensive check error or an error occurred when doing the
/// actual io operation.
///
/// It indicates a data crash.
/// An application returning this error will shutdown the Openraft node immediately to prevent
/// further damage.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize), serde(bound = ""))]
pub enum StorageError<C>
where C: RaftTypeConfig
{
    /// An error raised by defensive check.
    #[error(transparent)]
    Defensive {
        #[from]
        #[cfg_attr(feature = "bt", backtrace)]
        source: DefensiveError<C>,
    },

    /// An error raised by io operation.
    #[error(transparent)]
    IO {
        #[from]
        #[cfg_attr(feature = "bt", backtrace)]
        source: StorageIOError<C>,
    },
}

impl<C> StorageError<C>
where C: RaftTypeConfig
{
    pub fn into_defensive(self) -> Option<DefensiveError<C>> {
        match self {
            StorageError::Defensive { source } => Some(source),
            _ => None,
        }
    }

    pub fn into_io(self) -> Option<StorageIOError<C>> {
        match self {
            StorageError::IO { source } => Some(source),
            _ => None,
        }
    }

    pub fn from_io_error(subject: ErrorSubject<C>, verb: ErrorVerb, io_error: std::io::Error) -> Self {
        let sto_io_err = StorageIOError::new(subject, verb, AnyError::new(&io_error));
        StorageError::IO { source: sto_io_err }
    }
}

/// Error that occurs when operating the store.
///
/// It indicates a data crash.
/// An application returning this error will shutdown the Openraft node immediately to prevent
/// further damage.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize), serde(bound = ""))]
pub struct StorageIOError<C>
where C: RaftTypeConfig
{
    subject: ErrorSubject<C>,
    verb: ErrorVerb,
    source: AnyError,
    backtrace: Option<String>,
}

impl<C> fmt::Display for StorageIOError<C>
where C: RaftTypeConfig
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "when {:?} {:?}: {}", self.verb, self.subject, self.source)
    }
}

impl<C> StorageIOError<C>
where C: RaftTypeConfig
{
    pub fn new(subject: ErrorSubject<C>, verb: ErrorVerb, source: impl Into<AnyError>) -> Self {
        Self {
            subject,
            verb,
            source: source.into(),
            backtrace: anyerror::backtrace_str(),
        }
    }

    pub fn write_log_entry(log_id: LogId<C::NodeId>, source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Log(log_id), ErrorVerb::Write, source)
    }

    pub fn read_log_entry(log_id: LogId<C::NodeId>, source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Log(log_id), ErrorVerb::Read, source)
    }

    pub fn write_logs(source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Logs, ErrorVerb::Write, source)
    }

    pub fn read_logs(source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Logs, ErrorVerb::Read, source)
    }

    pub fn write_vote(source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Vote, ErrorVerb::Write, source)
    }

    pub fn read_vote(source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Vote, ErrorVerb::Read, source)
    }

    pub fn apply(log_id: LogId<C::NodeId>, source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Apply(log_id), ErrorVerb::Write, source)
    }

    pub fn write_state_machine(source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::StateMachine, ErrorVerb::Write, source)
    }

    pub fn read_state_machine(source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::StateMachine, ErrorVerb::Read, source)
    }

    pub fn write_snapshot(signature: Option<SnapshotSignature<C>>, source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Snapshot(signature), ErrorVerb::Write, source)
    }

    pub fn read_snapshot(signature: Option<SnapshotSignature<C>>, source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Snapshot(signature), ErrorVerb::Read, source)
    }

    /// General read error
    pub fn read(source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Store, ErrorVerb::Read, source)
    }

    /// General write error
    pub fn write(source: impl Into<AnyError>) -> Self {
        Self::new(ErrorSubject::Store, ErrorVerb::Write, source)
    }
}
