use crate::session::format_session_list;

pub fn execute_list(current_session_id: Option<&str>) {
    let sessions = crate::session::Session::list_all();
    let output = format_session_list(&sessions, current_session_id);
    println!("{}", output);
}
