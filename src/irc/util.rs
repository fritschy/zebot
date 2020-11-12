use std::borrow::Cow;

pub trait Join<T, S = T> {
    fn join(&self, sep: S) -> T;
}

impl<'a> Join<String, &str> for Vec<Cow<'a, str>> {
    fn join(&self, sep: &str) -> String {
        self.iter().fold(String::new(), |acc, x| {
            if acc.is_empty() {
                x.to_string()
            } else {
                acc + sep + x
            }
        })
    }
}
