use super::*;

/// REQ-PERS-005: on a terminal a carriage return that ends no line is shown as `\r`, so that
/// no text can hide what precedes it on its line; `\r\n` (a line ending) and a text without a
/// carriage return are untouched.
#[test]
fn req_pers_005_a_carriage_return_that_ends_no_line_is_shown() {
    assert_eq!(show_carriage_returns("plain\n"), "plain\n");
    assert_eq!(show_carriage_returns("a\r\nb\r\n"), "a\r\nb\r\n");
    assert_eq!(
        show_carriage_returns("progress 50%\rprogress 100%\n"),
        "progress 50%\\rprogress 100%\n"
    );
    // A carriage return at the very end ends no line either; two in a row: only the last may end one.
    assert_eq!(show_carriage_returns("a\r"), "a\\r");
    assert_eq!(show_carriage_returns("a\r\r\nb"), "a\\r\r\nb");
    assert_eq!(show_carriage_returns("\r"), "\\r");
    assert_eq!(show_carriage_returns(""), "");
    // What is hidden on a terminal is what a reader would not see: nothing of the line is lost.
    let hiding = "the password is hunter2\rall is well          \n";
    let shown = show_carriage_returns(hiding);
    assert!(shown.contains("hunter2") && shown.contains("\\r"));
    assert!(!shown.contains('\r'));
}

/// REQ-PERS-005: piped, the text is exact; on a terminal the carriage returns are shown and the
/// last line is ended (so that the prompt that follows is not written after it), an empty text
/// stays empty.
#[test]
fn req_pers_005_the_text_is_exact_unless_a_terminal_reads_it() {
    let text = "a\rb\r\nc";
    assert_eq!(text_to_print(text, false), text);
    assert_eq!(text_to_print(text, true), "a\\rb\r\nc\n");
    assert_eq!(text_to_print("ends\n", true), "ends\n");
    assert_eq!(text_to_print("ends\r", true), "ends\\r\n");
    assert_eq!(text_to_print("", true), "");
    assert_eq!(text_to_print("", false), "");
    assert_eq!(text_to_print("no newline", false), "no newline");
}
