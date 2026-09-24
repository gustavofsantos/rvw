package tui

import (
	"fmt"
	"os"
	"os/exec"
	"strings"
)

// scissors separates what the user writes from the context below it, as in
// `git commit --cleanup=scissors`.
const scissors = "# ------------------------ >8 ------------------------"

// maxContextLines caps the code quoted into the temp file.
const maxContextLines = 40

// template is the temp file the editor opens: the text to edit, then the
// scissors line and #-prefixed context.
func template(body string, context []string) string {
	var b strings.Builder
	b.WriteString(body)
	if !strings.HasSuffix(body, "\n") {
		b.WriteString("\n")
	}
	b.WriteString("\n" + scissors + "\n")
	b.WriteString("# Do not modify or remove the line above.\n")
	b.WriteString("# Everything below it is ignored. Save an empty note to cancel.\n")
	for _, line := range context {
		b.WriteString(strings.TrimRight("# "+line, " ") + "\n")
	}
	return b.String()
}

// stripTemplate reads back what the user wrote: everything above the scissors
// line, or, when that line was removed, every line not starting with '#'.
func stripTemplate(text string) string {
	lines := strings.Split(strings.ReplaceAll(text, "\r\n", "\n"), "\n")
	var kept []string
	cut := false
	for i, l := range lines {
		if l == scissors {
			kept, cut = lines[:i], true
			break
		}
	}
	if !cut {
		for _, l := range lines {
			if !strings.HasPrefix(l, "#") {
				kept = append(kept, l)
			}
		}
	}
	return strings.TrimSpace(strings.Join(kept, "\n"))
}

// quoteCode is the context lines for a range of code, numbered.
func quoteCode(lines []string, start int) []string {
	out := []string{""}
	width := len(fmt.Sprint(start + len(lines) - 1))
	for i, l := range lines {
		if i == maxContextLines {
			out = append(out, fmt.Sprintf("  %*s  … %d more lines", width, "", len(lines)-i))
			break
		}
		out = append(out, fmt.Sprintf("  %*d  %s", width, start+i, l))
	}
	return out
}

// writeTemp writes the editor's temp file and returns its path.
func writeTemp(content string) (string, error) {
	f, err := os.CreateTemp("", "rvw-*.md")
	if err != nil {
		return "", err
	}
	if _, err := f.WriteString(content); err != nil {
		f.Close()
		os.Remove(f.Name())
		return "", err
	}
	return f.Name(), f.Close()
}

// editorCmd runs the editor command line on a file through the shell, so an
// editor given with arguments ("code --wait") works, as git does.
func editorCmd(editor, path string) *exec.Cmd {
	return exec.Command("sh", "-c", editor+` "$@"`, editor, path)
}
