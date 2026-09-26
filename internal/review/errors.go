package review

import (
	"errors"
	"fmt"
)

// ErrorKind classifies a failure so each adapter can map it: the CLI prints the
// message and exits 1, an MCP tool can return it as a typed tool error.
type ErrorKind string

const (
	KindInvalid  ErrorKind = "invalid"   // the input is malformed or incomplete
	KindNotFound ErrorKind = "not_found" // an id or file does not exist
	KindConflict ErrorKind = "conflict"  // the request is valid but the current state forbids it
	KindInternal ErrorKind = "internal"  // storage or git failed underneath
)

// Error is every failure the service reports on purpose.
type Error struct {
	Kind    ErrorKind
	Message string
}

func (e *Error) Error() string { return e.Message }

// KindOf returns the kind of err, or [KindInternal] for an unclassified error.
func KindOf(err error) ErrorKind {
	if e, ok := errors.AsType[*Error](err); ok {
		return e.Kind
	}
	return KindInternal
}

func newError(kind ErrorKind, format string, args ...any) *Error {
	return &Error{Kind: kind, Message: fmt.Sprintf(format, args...)}
}

// Invalidf reports malformed input.
func Invalidf(format string, args ...any) error { return newError(KindInvalid, format, args...) }

func notFoundf(format string, args ...any) error { return newError(KindNotFound, format, args...) }
func conflictf(format string, args ...any) error { return newError(KindConflict, format, args...) }
func internalf(format string, args ...any) error { return newError(KindInternal, format, args...) }
