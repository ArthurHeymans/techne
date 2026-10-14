const text = require("fs").readFileSync("org-sample.org", "utf8");
const lines = text.split("\n"); if (lines[lines.length - 1] === "") lines.pop();
const STAR = 42, SP = 32, COLON = 58;
function run() {
  const tags = new Map(), levels = [0, 0, 0, 0, 0, 0, 0, 0];
  let todos = 0, opn = 0, done = 0, words = 0;
  for (const line of lines) {
    const len = line.length; let stars = 0;
    while (stars < len && line.charCodeAt(stars) === STAR) stars++;
    if (stars > 0 && stars < 8 && stars < len && line.charCodeAt(stars) === SP) {
      levels[stars]++;
      if (line.startsWith("TODO ", stars + 1)) todos++;
      if (line.charCodeAt(len - 1) === COLON) {
        let i = len - 1;
        while (i >= 0 && line.charCodeAt(i) !== SP) i--;
        if (i >= 0) {
          let start = i + 1;
          for (let j = i + 1; j < len; j++) {
            if (line.charCodeAt(j) === COLON) {
              if (j > start) { const t = line.substring(start, j); tags.set(t, (tags.get(t) || 0) + 1); }
              start = j + 1;
            }
          }
        }
      }
    } else if (line.startsWith("- [ ] ")) opn++;
    else if (line.startsWith("- [X] ")) done++;
    else {
      let inw = false;
      for (let i = 0; i < len; i++) { const sp = line.charCodeAt(i) === SP; if (!sp && !inw) words++; inw = !sp; }
    }
  }
  return [levels[1], levels[2], levels[3], todos, opn, done, words, tags.size, tags.get("work") || 0].join(" ");
}
