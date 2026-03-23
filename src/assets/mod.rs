pub struct Frontend;

impl Frontend {
    pub fn get_index() -> Option<&'static str> {
        Some(include_str!("../../frontend/index.html"))
    }
}
