//! What a screen reader hears of new output: the lines output scrolled in,
//! what was added to a line, or a line that changed, not the whole screen
//! nor a whole line again for each character the shell echoes.

/// The text a reader is told of a screen going from `before` to `after`
/// (each row's text, top first): empty when nothing worth saying changed.
pub(crate) fn news(before: &[&str], after: &[&str]) -> String {
    let rows = after.len();
    let mut pieces: Vec<&str> = Vec::new();
    if before.len() == rows && before != after {
        // Output scrolled the screen by `by` lines: those rows moved up and
        // as many came in at the bottom.
        let scrolled = (1..rows).find(|by| after[..rows - by] == before[*by..]);
        match scrolled {
            Some(by) => pieces.extend(after[rows - by..].iter().copied()),
            None => {
                for (was, now) in before.iter().zip(after) {
                    if was == now {
                        continue;
                    }
                    // A line grown at its end (an echo, a progress counter)
                    // says what it gained.
                    pieces.push(now.strip_prefix(*was).unwrap_or(now));
                }
            }
        }
    }
    joined(pieces)
}

/// `news` for a screen whose rows didn't move, of which only some may have
/// changed: each row's text before and now. What the rest showed isn't
/// looked at, so a line streaming in at the bottom of a tall screen costs
/// that line, not the screen.
pub(crate) fn changed_news<'a>(rows: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    joined(
        rows.into_iter()
            .filter(|(was, now)| was != now)
            // A line grown at its end says what it gained (as `news`).
            .map(|(was, now)| now.strip_prefix(was).unwrap_or(now)),
    )
}

/// `pieces` trimmed, the blank ones left out, a line each: joined at
/// once, its length known, rather than grown a piece at a time.
fn joined<'a>(pieces: impl IntoIterator<Item = &'a str>) -> String {
    let pieces: Vec<&str> = pieces
        .into_iter()
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .collect();
    pieces.join("\n")
}

/// `news` less the shell's echo of what was `typed` (and not yet echoed),
/// which a reader's own key echo has said already. What was typed waits for
/// its echo: a read showing nothing new (the cursor's row drawn again)
/// leaves it, and an echo of part of it leaves the rest, or the echo after
/// it was read out a character at a time. News that isn't an echo (the
/// command's output) is said, and ends the wait.
pub(crate) fn less_echo(typed: &mut String, news: String) -> String {
    if news.is_empty() {
        return news;
    }
    // What wasn't typed can't be its echo: output longer than all that
    // waits for one is said without searching for it there.
    if news.len() > typed.len() {
        typed.clear();
        return news;
    }
    match typed.find(news.as_str()) {
        Some(at) => {
            typed.drain(..at + news.len());
            String::new()
        }
        None => {
            typed.clear();
            news
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{less_echo, news};

    #[test]
    fn the_echo_of_what_was_typed_is_not_news() {
        let mut typed = String::from("ls -l");
        // A read with nothing new keeps what was typed for its echo.
        assert_eq!(less_echo(&mut typed, String::new()), "");
        assert_eq!(typed, "ls -l");
        // The echo a character, then the rest, at a time: none of it said.
        assert_eq!(less_echo(&mut typed, "l".into()), "");
        assert_eq!(less_echo(&mut typed, "s".into()), "");
        assert_eq!(less_echo(&mut typed, "-l".into()), "");
        assert_eq!(typed, "");
        // The command's output is said, and nothing typed is waited for.
        typed.push('x');
        assert_eq!(less_echo(&mut typed, "a.txt".into()), "a.txt");
        assert_eq!(typed, "");
        assert_eq!(less_echo(&mut typed, "x".into()), "x");
    }

    /// Rows that didn't all change didn't scroll: only those changed are
    /// news, each as `news` says a changed row, top first. On a screen of
    /// repeated lines, `news` (which must look for a scroll) can take a
    /// changed bottom row for lines scrolled in.
    #[test]
    fn a_reader_hears_only_the_rows_that_changed() {
        assert_eq!(super::changed_news([("$ ls", "$ ls -l")]), "-l");
        assert_eq!(
            super::changed_news([("", "a.txt"), ("same", "same"), ("", "b.txt")]),
            "a.txt\nb.txt"
        );
        assert_eq!(
            super::changed_news([("[==  ] 40%", "[=== ] 60%")]),
            "[=== ] 60%"
        );
        assert_eq!(super::changed_news([("a", "a"), ("b", "  ")]), "");
        assert_eq!(news(&["x", "y", "x", "y"], &["x", "y", "x", "z"]), "x\nz");
        assert_eq!(super::changed_news([("y", "z")]), "z");
    }

    /// Output longer than what waits for its echo is said as it is: none
    /// of it could be the echo.
    #[test]
    fn output_longer_than_what_was_typed_is_said() {
        let mut typed = String::from("ls");
        assert_eq!(less_echo(&mut typed, "a.txt b.txt".into()), "a.txt b.txt");
        assert_eq!(typed, "");
        let mut typed = String::from("ls -l");
        assert_eq!(less_echo(&mut typed, "ls".into()), "");
        assert_eq!(typed, " -l");
    }

    #[test]
    fn a_reader_hears_what_is_new() {
        // An echo: what the line gained.
        assert_eq!(news(&["$ ls", ""], &["$ ls -l", ""]), "-l");
        // Output on empty rows below the prompt.
        assert_eq!(
            news(&["$ ls", "", ""], &["$ ls", "a.txt", "b.txt"]),
            "a.txt\nb.txt"
        );
        // A full screen scrolled by two: the two lines that came in.
        assert_eq!(news(&["1", "2", "3", "4"], &["3", "4", "5", "6"]), "5\n6");
        // A line rewritten in place (a progress bar) whole.
        assert_eq!(news(&["[==  ] 40%", ""], &["[=== ] 60%", ""]), "[=== ] 60%");
        // Nothing new, or only blanks.
        assert_eq!(news(&["a", "b"], &["a", "b"]), "");
        assert_eq!(news(&["a", "b"], &["a", ""]), "");
    }
}
