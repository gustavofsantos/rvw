package tui

import "github.com/gustavofsantos/rvw/internal/review"

// Rail glyphs mark every line of a comment's range in the gutter.
const (
	railSingle = "●"
	railFirst  = "╭"
	railMiddle = "│"
	railLast   = "╰"
)

// railAt picks the comment whose rail is drawn on a 1-indexed line: the most
// recent of those covering it (comments come oldest first). ok is false for
// an uncovered line.
func railAt(cs []review.Comment, line int) (c review.Comment, glyph string, ok bool) {
	for i := len(cs) - 1; i >= 0; i-- {
		c := cs[i]
		if covers(c, line) {
			return c, railGlyph(c, line), true
		}
	}
	return review.Comment{}, "", false
}

func railGlyph(c review.Comment, line int) string {
	switch {
	case c.StartLine == c.EndLine:
		return railSingle
	case line == c.StartLine:
		return railFirst
	case line == c.EndLine:
		return railLast
	}
	return railMiddle
}

func covers(c review.Comment, line int) bool { return c.StartLine <= line && line <= c.EndLine }

// covering lists every comment on a line, oldest first.
func covering(cs []review.Comment, line int) []review.Comment {
	var out []review.Comment
	for _, c := range cs {
		if covers(c, line) {
			out = append(out, c)
		}
	}
	return out
}

// nextComment is the first line of the nearest comment starting after line
// (dir 1) or before it (dir -1); ok is false when there is none.
func nextComment(cs []review.Comment, line, dir int) (int, bool) {
	starts := make([]int, len(cs))
	for i, c := range cs {
		starts[i] = c.StartLine
	}
	return nearest(starts, line, dir)
}

// nearest is the closest of lines after line (dir 1) or before it (dir -1);
// ok is false when there is none.
func nearest(lines []int, line, dir int) (int, bool) {
	best, ok := 0, false
	for _, s := range lines {
		if (dir > 0 && s > line || dir < 0 && s < line) && (!ok || (s-best)*dir < 0) {
			best, ok = s, true
		}
	}
	return best, ok
}
