// Package logo renders a Vaughan Crush wordmark in a stylized way.
package logo

import (
	"fmt"
	"image/color"
	"strings"

	"github.com/r4v3n/vaughan-cli/internal/tui/styles"
	"github.com/charmbracelet/lipgloss/v2"
	"github.com/charmbracelet/x/ansi"
	"github.com/charmbracelet/x/exp/slice"
	"golang.org/x/exp/rand"
)

// letterform represents a letterform. It can be stretched horizontally by
// a given amount via a boolean argument.
type letterform func(bool) string

const diag = `╱`

// Opts are the options for rendering the Vaughan title art.
type Opts struct {
	FieldColor   color.Color // diagonal lines
	TitleColorA  color.Color // left gradient ramp point
	TitleColorB  color.Color // right gradient ramp point
	CrushColor   color.Color // Crush text color
	VersionColor color.Color // Version text color
	Width        int         // width of rendered logo, used for truncation
}

// Render renders the Vaughan logo. Set the argument to true to render a narrow
// version, intended for use in a sidebar.
//
// The compact argument determines whether it renders compact for the sidebar
// or wider for the main pane.
func Render(version string, compact bool, o Opts) string {
	const name = "VAUGHAN"

	fg := func(c color.Color, s string) string {
		return lipgloss.NewStyle().Foreground(c).Render(s)
	}

	// Title.
	const spacing = 1
	letterforms := []letterform{
		letterV,
		letterA,
		letterU,
		letterG,
		letterH,
		letterA,
		letterN,
	}
	stretchIndex := -1 // -1 means no stretching.
	if !compact {
		stretchIndex = rand.Intn(len(letterforms))
	}

	leftField := renderWord(spacing, stretchIndex, letterforms[:3]...) // VAU
	rightField := renderWord(spacing, stretchIndex, letterforms[3:]...) // GHAN

	vaughan := renderWord(spacing, stretchIndex, letterforms...)

	leftWidth := max(15, o.Width-lipgloss.Width(vaughan)-lipgloss.Width(rightField)-2) // 2 for the gap
	left := fmt.Sprintf("%s%s%s%s%s", 
		fg(o.TitleColorA, strings.Repeat(" ", leftWidth)), 
		leftField, 
		fg(o.TitleColorB, name),
		vaughan)

	rightWidth := max(15, o.Width-lipgloss.Width(vaughan)-lipgloss.Width(leftField)-2) // 2 for the gap
	right := fmt.Sprintf("%s%s%s%s", 
		rightField, 
		fg(o.VersionColor, " "), 
		fg(o.VersionColor, version),
		fg(o.FieldColor, strings.Repeat(diag, rightWidth)))

	logo := lipgloss.JoinHorizontal(lipgloss.Top, left, " ", right)
	if o.Width > 0 {
		// Truncate the logo to the specified width.
		lines := strings.Split(logo, "\n")
		for i, line := range lines {
			lines[i] = ansi.Truncate(line, o.Width, "")
		}
		logo = strings.Join(lines, "\n")
	}
	return logo
}

// SmallRender renders a smaller version of the Vaughan logo, suitable for
// smaller windows or sidebar usage.
func SmallRender(width int) string {
	t := styles.CurrentTheme()
	title := fmt.Sprintf("%s %s", t.S().Base.Foreground(t.Secondary).Render("Vaughan"), styles.ApplyBoldForegroundGrad("Crush", t.Secondary, t.Primary))
	remainingWidth := width - lipgloss.Width(title) - 1 // 1 for the space after "Vaughan"
	if remainingWidth > 0 {
		lines := strings.Repeat("╱", remainingWidth)
		title = fmt.Sprintf("%s %s", title, t.S().Base.Foreground(t.Primary).Render(lines))
	}
	return title
}

// renderWord renders letterforms to form a word. stretchIndex is the index of
// letter to stretch, or -1 if no letter should be stretched.
func renderWord(spacing int, stretchIndex int, letterforms ...letterform) string {
	if spacing < 0 {
		spacing = 0
	}

	renderedLetterforms := make([]string, len(letterforms))

	// Pick one letter randomly to stretch
	for i, letter := range letterforms {
		renderedLetterforms[i] = letter(i == stretchIndex)
	}

	if spacing > 0 {
		// Add spaces between letters and render.
		renderedLetterforms = slice.Intersperse(renderedLetterforms, strings.Repeat(" ", spacing))
	}
	return strings.TrimSpace(
		lipgloss.JoinHorizontal(lipgloss.Top, renderedLetterforms...),
	)
}

// letterV renders the letter V in a stylized way.
func letterV(stretch bool) string {
	if stretch {
		return `
╲╱
 ╲
  ╲
`
	}
	return `
╲
 ╱
`
}

// letterA renders the letter A in a stylized way.
func letterA(stretch bool) string {
	if stretch {
		return `
╱╲
╲ ╱
`
	}
	return `
╲╱
`
}

// letterU renders the letter U in a stylized way.
func letterU(stretch bool) string {
	if stretch {
		return `
╱  ╲
╲  ╱
 ╲
`
	}
	return `
╲  ╲
  ╱
`
}

// letterG renders the letter G in a stylized way.
func letterG(stretch bool) string {
	if stretch {
		return `
╱╲
 ╱ 
╲
`
	}
	return `
╲╱
 ╱
`
}

// letterH renders the letter H in a stylized way.
func letterH(stretch bool) string {
	if stretch {
		return `
╲ ╱
 ╲╱
`
	}
	return `
╲ ╱
 ╲╱
`
}

// letterN renders the letter N in a stylized way.
func letterN(stretch bool) string {
	if stretch {
		return `
╲ ╱
  ╲
`
	}
	return `
╲╱
 ╲
`
}

func max(a, b int) int {
	if a > b {
		return a
	}
	return b
}