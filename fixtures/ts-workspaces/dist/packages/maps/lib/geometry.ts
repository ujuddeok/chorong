export interface Shape {
  area(): number;
}

export function square(side: number): Shape {
  return { area: () => side * side };
}
