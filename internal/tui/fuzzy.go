package tui

import (
	"cmp"
	"slices"
	"strings"
	"unicode"
)

// Scores of the fzf-style matcher. Every matched character earns scoreMatch
// plus a bonus for where it lands; a run of consecutive matches keeps the
// bonus of the boundary it started on. Gaps between matches cost.
const (
	scoreMatch       = 16
	bonusSegment     = 12 // the start of a path segment: after '/', or first
	bonusBoundary    = 8  // after another separator
	bonusCamel       = 7  // an upper-case letter after a lower-case one
	bonusConsecutive = 6  // right after the previous matched character
	bonusFileName    = 2  // inside the last path segment
	penaltyGapStart  = 3
	penaltyGapExtend = 1
)

const negInf = -1 << 30

// fuzzyMatch scores text against a pattern whose characters must appear in
// order, case-insensitively. It returns the best alignment's score and the
// rune indices it matched.
func fuzzyMatch(pattern, text string) (int, []int, bool) {
	p := []rune(strings.ToLower(pattern))
	t := []rune(text)
	n, m := len(t), len(p)
	if m == 0 {
		return 0, nil, true
	}
	if m > n || !subsequence(p, t) {
		return 0, nil, false
	}
	lower := make([]rune, n)
	bonus := make([]int, n)
	name := make([]int, n)
	nameStart := len([]rune(text[:strings.LastIndex(text, "/")+1]))
	for j, r := range t {
		lower[j] = unicode.ToLower(r)
		bonus[j] = positionBonus(t, j)
		if j >= nameStart {
			name[j] = bonusFileName
		}
	}

	// score[i][j]: best score with p[i] matched at t[j]; from[i][j]: where
	// p[i-1] went; chunk[i][j]: the bonus the run ending there started with.
	score := make([][]int, m)
	from := make([][]int, m)
	chunk := make([][]int, m)
	for i := range m {
		score[i] = make([]int, n)
		from[i] = make([]int, n)
		chunk[i] = make([]int, n)
		best, bestAt := negInf, -1 // best score[i-1][k] minus its gap, for k < j-1
		for j := range n {
			if i > 0 && j >= 2 {
				best -= penaltyGapExtend
				if s := score[i-1][j-2] - penaltyGapStart; s > best {
					best, bestAt = s, j-2
				}
			}
			score[i][j] = negInf
			if lower[j] != p[i] {
				continue
			}
			fresh := scoreMatch + bonus[j] + name[j]
			if i == 0 {
				score[i][j], chunk[i][j] = fresh, bonus[j]
				continue
			}
			if best > negInf/2 {
				score[i][j], from[i][j], chunk[i][j] = best+fresh, bestAt, bonus[j]
			}
			if j > 0 && score[i-1][j-1] > negInf/2 {
				first := chunk[i-1][j-1]
				if bonus[j] >= bonusBoundary && bonus[j] > first {
					first = bonus[j]
				}
				s := score[i-1][j-1] + scoreMatch + max(bonus[j], first, bonusConsecutive) + name[j]
				if s >= score[i][j] {
					score[i][j], from[i][j], chunk[i][j] = s, j-1, first
				}
			}
		}
	}

	end, total := -1, negInf
	for j := range n {
		if score[m-1][j] > total {
			total, end = score[m-1][j], j
		}
	}
	if end < 0 || total <= negInf/2 {
		return 0, nil, false
	}
	pos := make([]int, m)
	for i := m - 1; i >= 0; i-- {
		pos[i] = end
		end = from[i][end]
	}
	return total, pos, true
}

func subsequence(p, t []rune) bool {
	i := 0
	for _, r := range t {
		if i < len(p) && unicode.ToLower(r) == p[i] {
			i++
		}
	}
	return i == len(p)
}

func positionBonus(t []rune, j int) int {
	if j == 0 {
		return bonusSegment
	}
	prev, cur := t[j-1], t[j]
	switch {
	case prev == '/':
		return bonusSegment
	case strings.ContainsRune("_-. :", prev):
		return bonusBoundary
	case unicode.IsLower(prev) && unicode.IsUpper(cur):
		return bonusCamel
	}
	return 0
}

// candidate is one row a picker can show.
type candidate struct {
	label    string // what is matched and shown
	comments int    // open comments, a tie-breaker
}

// ranked is a candidate that matched, with where.
type ranked struct {
	index int
	score int
	pos   []int
}

// rank filters candidates by the query and orders them best first: score,
// then files with open comments, then shorter labels, then input order.
func rank(query string, cands []candidate) []ranked {
	out := []ranked{}
	for i, c := range cands {
		score, pos, ok := fuzzyMatch(query, c.label)
		if ok {
			out = append(out, ranked{index: i, score: score, pos: pos})
		}
	}
	if query == "" {
		return out
	}
	slices.SortStableFunc(out, func(a, b ranked) int {
		ca, cb := cands[a.index], cands[b.index]
		return cmp.Or(
			cmp.Compare(b.score, a.score),
			cmp.Compare(min(cb.comments, 1), min(ca.comments, 1)),
			cmp.Compare(len(ca.label), len(cb.label)),
			cmp.Compare(a.index, b.index),
		)
	})
	return out
}
