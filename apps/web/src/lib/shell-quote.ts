// POSIX-style single-quote shell escaping. Bare path when nothing dangerous;
// otherwise wrap in single quotes and escape embedded single quotes via the
// standard `'\''` terminate-escape-restart trick. Backslashes (Windows paths)
// are preserved verbatim by single-quote rules.
export function shellQuote(path: string): string {
  if (!/[\s'"\\$`(){}[\]<>|&;*?#~!]/.test(path)) return path
  return `'${path.replace(/'/g, `'\\''`)}'`
}
