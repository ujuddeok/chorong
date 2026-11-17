¥import a from 'a';
// note
import { b,
  c } from 'b';
import * as d from 'd';
export const f = async <T,>(
  x: T,
  y: number,
): Promise<T> => {
  if (x) {
    return x;
  } else if (y) {
    return `${
      y
    }`;
  } else {
    throw new Error(
      "no",
    );
  }
};
@Component({
  selector: 'x',
  template: '<div></div>',
}) export class Q<T> extends Base {
  @Input() set v(value: T) {
    this.a = value;
    this.b = [
      1, 2,
    ];
  }
  method(): void { call(a, (b) => {
    return b;
  }); }
}
function g() {
  return (
    <div>
      {items.map((i) => (
        <span key={i}>{i}</span>
      ))}
    </div>
  );
}
