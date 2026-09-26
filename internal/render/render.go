// Package render turns service outputs into what a terminal, an editor or an
// agent reads: one-line text, markdown handoffs, JSON envelopes and the
// fixed-width display sheets.
package render

import (
	"encoding/json"
	"fmt"
	"io"
	"strings"

	"github.com/gustavofsantos/rvw/internal/review"
)

// JSON writes v indented, without escaping <, > and & inside code snippets.
func JSON(w io.Writer, v any) error {
	enc := json.NewEncoder(w)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", "  ")
	return enc.Encode(v)
}

// Envelope is the JSON shape of `list` and `pull`: comments under "reviews" and
// submitted reviews under "submitted_reviews", kept for existing consumers.
type Envelope struct {
	Workspace        string           `json:"workspace"`
	Count            int              `json:"count"`
	Comments         []review.Comment `json:"reviews"`
	SubmittedReviews []review.Handoff `json:"submitted_reviews,omitempty"`
}

// FirstLine is the first line of text, trimmed of surrounding blank lines.
func FirstLine(text string) string {
	line, _, _ := strings.Cut(strings.TrimSpace(text), "\n")
	return line
}

// Attribution is `@author #lane`, omitting whichever is absent.
func Attribution(author, lane *string) string {
	var bits []string
	if author != nil && *author != "" {
		bits = append(bits, "@"+*author)
	}
	if lane != nil && *lane != "" {
		bits = append(bits, "#"+*lane)
	}
	return strings.Join(bits, " ")
}

// CommentLine is the one-line summary of a comment: `r3  file:2-4  @who  note`.
func CommentLine(c review.Comment, idWidth int) string {
	line := fmt.Sprintf("%-*s  %s  ", idWidth, c.ID, c.Location())
	if who := Attribution(c.Author, c.Lane); who != "" {
		line += who + "  "
	}
	return line + FirstLine(c.Comment)
}

// ReviewLine is the one-line summary of a submitted review.
func ReviewLine(r review.Review) string {
	return fmt.Sprintf("%s  %s  %d review comment(s)  %s", r.ID, r.Decision, len(r.CommentIDs), FirstLine(r.Summary))
}

// SheetLine is the one-line summary of a review sheet:
// `rv1  [pending]  approve  2 review comment(s)  @who  summary`.
func SheetLine(rs review.ReviewSheet) string {
	r := rs.Review
	line := fmt.Sprintf("%s  [%s]  %s  %d review comment(s)  ", r.ID, rs.State, r.Decision, len(r.CommentIDs))
	if who := Attribution(r.Author, r.Lane); who != "" {
		line += who + "  "
	}
	return line + FirstLine(r.Summary)
}

// Text writes handoffs one per line, reviews first. It reports whether it
// wrote anything.
func Text(w io.Writer, reviews []review.Handoff, comments []review.Comment) bool {
	for _, r := range reviews {
		fmt.Fprintln(w, ReviewLine(r.Review))
	}
	width := 0
	for _, c := range comments {
		width = max(width, len(c.ID))
	}
	for _, c := range comments {
		fmt.Fprintln(w, CommentLine(c, width))
	}
	return len(reviews)+len(comments) > 0
}

// IDs writes one id per line, reviews first.
func IDs(w io.Writer, reviews []review.Handoff, comments []review.Comment) {
	for _, r := range reviews {
		fmt.Fprintln(w, r.ID)
	}
	for _, c := range comments {
		fmt.Fprintln(w, c.ID)
	}
}

// Markdown is the handoff an agent reads: each comment's location, the code as
// it stood, and the note as a quote.
func Markdown(ws string, reviews []review.Handoff, comments []review.Comment, drained bool) string {
	if len(reviews) == 0 && len(comments) == 0 {
		return fmt.Sprintf("No pending review comments for %s.\n", ws)
	}
	var out []string
	for _, r := range reviews {
		out = append(out,
			fmt.Sprintf("# Review %s — %s", r.ID, ws), "",
			"**Decision:** "+decisionLabel(r.Decision), "",
			r.Summary, "",
			"## Review comments", "")
		for i, c := range r.Comments {
			out = appendCommentMarkdown(out, "###", i+1, c)
		}
	}
	if len(comments) > 0 {
		out = append(out,
			"# Review comments — "+ws, "",
			"Address the following review comments, in the order listed. Each names a file",
			"and line range, the code as it stood when the comment was written, and the",
			"reviewer's note as a quote.", "")
	}
	if drained {
		out = append(out,
			"These comments have been dequeued — they will not appear again. Record what",
			"became of each one with `rvw resolve <id> --note ...`, or",
			"`rvw reject <id> --note ...` when you are not making the change.", "")
	}
	for i, c := range comments {
		out = appendCommentMarkdown(out, "##", i+1, c)
	}
	return strings.TrimRight(strings.Join(out, "\n"), " \t\n") + "\n"
}

func appendCommentMarkdown(out []string, heading string, n int, c review.Comment) []string {
	head := fmt.Sprintf("%s %d. `%s` (%s", heading, n, c.Location(), c.ID)
	if who := Attribution(c.Author, c.Lane); who != "" {
		head += " · " + who
	}
	out = append(out, head+")", "", "```"+c.Filetype)
	out = append(out, strings.Split(c.Code, "\n")...)
	out = append(out, "```", "")
	for line := range strings.SplitSeq(c.Comment, "\n") {
		if line == "" {
			out = append(out, ">")
		} else {
			out = append(out, "> "+line)
		}
	}
	return append(out, "")
}

// decisionLabel is "Request changes" for "request-changes".
func decisionLabel(d review.Decision) string {
	s := strings.ReplaceAll(string(d), "-", " ")
	if s == "" {
		return s
	}
	return strings.ToUpper(s[:1]) + s[1:]
}
