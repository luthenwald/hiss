/** Groups items by the key each one maps to, keeping their order. */
export function groupBy<T, K>(items: T[], key: (item: T) => K): Map<K, T[]> {
  const groups = new Map<K, T[]>();
  for (const item of items) {
    <|>
  }
  return groups;
}
