pub(crate) fn split_path(path: &str) -> impl Iterator<Item = &str> + '_ {
    path.split('/').filter(|part| !part.is_empty())
}
