import palette from "./banner.palette.json";

// hey.food CLI banner — palette shared with the packaged Python renderer.
// Raw ANSI truecolor, zero dependencies.

function ansi(hex: string): string {
  const value = hex.replace("#", "");
  const r = parseInt(value.slice(0, 2), 16);
  const g = parseInt(value.slice(2, 4), 16);
  const b = parseInt(value.slice(4, 6), 16);
  return `\x1b[38;2;${r};${g};${b}m`;
}

const foreground = ansi(palette.foreground);
const accent = ansi(palette.accent);
const reset = "\x1b[0m";

export const BANNER =
  `${foreground}▄                      ▄▄▄                 ▄${reset}\n` +
  `${foreground}█      ▄▄▄  ▄   ▄    ▄█▄▄   ▄▄▄   ▄▄▄   ▄▄▄█${reset}\n` +
  `${foreground}█▀▀▀▄ █▄▄▄█ █   █     █    █   █ █   █ █   █${reset}\n` +
  `${foreground}█   █ ▀▄▄▄  ▀▄▄▄█ ${accent}██${foreground}  █    ▀▄▄▄▀ ▀▄▄▄▀ ▀▄▄▄█${reset}\n` +
  `${foreground}             ▄▄▄▀${reset}`;

export const BANNER_PLAIN = `▄                      ▄▄▄                 ▄
█      ▄▄▄  ▄   ▄    ▄█▄▄   ▄▄▄   ▄▄▄   ▄▄▄█
█▀▀▀▄ █▄▄▄█ █   █     █    █   █ █   █ █   █
█   █ ▀▄▄▄  ▀▄▄▄█ ██  █    ▀▄▄▄▀ ▀▄▄▄▀ ▀▄▄▄█
             ▄▄▄▀`;

export function printBanner(color: boolean = process.stdout.isTTY): void {
  console.log(color ? BANNER : BANNER_PLAIN);
}
