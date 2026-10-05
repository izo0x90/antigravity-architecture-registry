import { Base, Formatter, clean, load, shout } from "./helpers";
import { clean as tidy } from "./helpers";
import * as h from "./helpers";

export class Greeter extends Base implements Formatter {
  greet(name: string): string {
    return this.format(clean(name));
  }

  format(text: string): string {
    throw new Error("TODO: format");
  }

  describe(): string {
    return super.describe();
  }
}

export function main(): void {
  const raw = load();
  new Greeter().greet(raw);
  show(raw);
  shout(raw);
}

function show(value: string): void {
  console.log(value);
}

export async function fetch(items: Greeter[], loud?: boolean, times = 1): Promise<string> {
  const later = () => h.load();
  if (false) {
    tidy("never");
  }
  for (const item of items) {
    item.greet("x");
  }
  return tidy("x");
}
