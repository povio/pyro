//! DOMException model. Names and legacy codes follow WebIDL
//! (https://webidl.spec.whatwg.org/#idl-DOMException-error-names).

use std::fmt;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DomErrorKind {
    IndexSize,
    HierarchyRequest,
    WrongDocument,
    InvalidCharacter,
    NoModificationAllowed,
    NotFound,
    NotSupported,
    InUseAttribute,
    InvalidState,
    Syntax,
    InvalidModification,
    Namespace,
    InvalidAccess,
    TypeMismatch,
    Security,
    Network,
    Abort,
    Timeout,
    DataClone,
    Encoding,
    NotAllowed,
    Operation,
}

impl DomErrorKind {
    pub const fn name(self) -> &'static str {
        use DomErrorKind::*;
        match self {
            IndexSize => "IndexSizeError",
            HierarchyRequest => "HierarchyRequestError",
            WrongDocument => "WrongDocumentError",
            InvalidCharacter => "InvalidCharacterError",
            NoModificationAllowed => "NoModificationAllowedError",
            NotFound => "NotFoundError",
            NotSupported => "NotSupportedError",
            InUseAttribute => "InUseAttributeError",
            InvalidState => "InvalidStateError",
            Syntax => "SyntaxError",
            InvalidModification => "InvalidModificationError",
            Namespace => "NamespaceError",
            InvalidAccess => "InvalidAccessError",
            TypeMismatch => "TypeMismatchError",
            Security => "SecurityError",
            Network => "NetworkError",
            Abort => "AbortError",
            Timeout => "TimeoutError",
            DataClone => "DataCloneError",
            Encoding => "EncodingError",
            NotAllowed => "NotAllowedError",
            Operation => "OperationError",
        }
    }

    /// Legacy `DOMException.code`; 0 for names introduced after the code table froze.
    pub const fn code(self) -> u16 {
        use DomErrorKind::*;
        match self {
            IndexSize => 1,
            HierarchyRequest => 3,
            WrongDocument => 4,
            InvalidCharacter => 5,
            NoModificationAllowed => 7,
            NotFound => 8,
            NotSupported => 9,
            InUseAttribute => 10,
            InvalidState => 11,
            Syntax => 12,
            InvalidModification => 13,
            Namespace => 14,
            InvalidAccess => 15,
            TypeMismatch => 17,
            Security => 18,
            Network => 19,
            Abort => 20,
            Timeout => 23,
            DataClone => 25,
            Encoding | NotAllowed | Operation => 0,
        }
    }
}

/// Error returned by DOM operations. `Type` maps to a JS `TypeError`
/// (WebIDL "simple exception"); everything else becomes a `DOMException`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DomError {
    Exception {
        kind: DomErrorKind,
        message: &'static str,
    },
    Type(&'static str),
    /// The handle refers to a node that has been collected (stale generation)
    /// or belongs to another `Dom` store. Bindings must surface this as an
    /// `InvalidStateError` rather than touching freed memory.
    StaleNode,
}

impl DomError {
    pub const fn new(kind: DomErrorKind, message: &'static str) -> Self {
        DomError::Exception { kind, message }
    }
    pub fn kind(&self) -> Option<DomErrorKind> {
        match self {
            DomError::Exception { kind, .. } => Some(*kind),
            DomError::StaleNode => Some(DomErrorKind::InvalidState),
            DomError::Type(_) => None,
        }
    }
}

impl fmt::Display for DomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DomError::Exception { kind, message } => write!(f, "{}: {}", kind.name(), message),
            DomError::Type(m) => write!(f, "TypeError: {m}"),
            DomError::StaleNode => f.write_str("InvalidStateError: node is no longer available"),
        }
    }
}

impl std::error::Error for DomError {}

pub type DomResult<T> = Result<T, DomError>;

pub(crate) const fn hierarchy(msg: &'static str) -> DomError {
    DomError::new(DomErrorKind::HierarchyRequest, msg)
}
pub(crate) const fn not_found(msg: &'static str) -> DomError {
    DomError::new(DomErrorKind::NotFound, msg)
}
