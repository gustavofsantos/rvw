package tui

import "github.com/gustavofsantos/rvw/internal/gitx"

// sign is a line's git change against HEAD, drawn left of its number.
type sign int

const (
	signNone       sign = iota
	signAdded           // a new line
	signChanged         // a line that replaced another
	signDeleted         // lines were deleted below this one
	signDeletedTop      // lines were deleted above this one, the first
)

var signGlyphs = [...]string{signNone: " ", signAdded: "▎", signChanged: "▎", signDeleted: "▁", signDeletedTop: "▔"}

// fileChanges is the change sign of each of a file's n lines, 0-indexed, and
// the line each hunk starts on, 1-indexed; both are nil when git has nothing
// to say: outside a worktree, without a HEAD commit, or on any git failure.
func fileChanges(dir, path string, n int) ([]sign, []int) {
	hunks, untracked, err := gitx.Changes(dir, path)
	if err != nil {
		return nil, nil
	}
	if untracked {
		hunks = []gitx.Hunk{{NewStart: 1, NewLines: n}}
	}
	return signs(hunks, n), hunkStarts(hunks, n)
}

// hunkStarts is the line each hunk is marked on first: a deletion's sign
// line, else its first new line.
func hunkStarts(hunks []gitx.Hunk, n int) []int {
	if n == 0 {
		return nil
	}
	var out []int
	for _, h := range hunks {
		out = append(out, clamp(h.NewStart, 1, n))
	}
	return out
}

// signs marks n lines with the hunks of a zero-context diff.
func signs(hunks []gitx.Hunk, n int) []sign {
	if len(hunks) == 0 || n == 0 {
		return nil
	}
	out := make([]sign, n)
	set := func(line int, s sign) { // 1-indexed
		if line >= 1 && line <= n {
			out[line-1] = s
		}
	}
	for _, h := range hunks {
		switch {
		case h.NewLines == 0 && h.NewStart == 0:
			set(1, signDeletedTop)
		case h.NewLines == 0:
			set(h.NewStart, signDeleted)
		default:
			s := signChanged
			if h.OldLines == 0 {
				s = signAdded
			}
			for l := h.NewStart; l < h.NewStart+h.NewLines; l++ {
				set(l, s)
			}
		}
	}
	return out
}
