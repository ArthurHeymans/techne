(define-library (techne terminal)
  (export make-terminal terminal-feed! terminal-resize! terminal-reset!
          terminal-rows terminal-columns terminal-row terminal-x terminal-y
          terminal-mode? terminal-title terminal-alternate?
          terminal-history-start terminal-history-end terminal-history-row terminal-history-slice
          terminal-row-cells terminal-row-cache terminal-row-cache-set! terminal-row-double-width?
          terminal-row-ascii terminal-row-plain terminal-row-columns terminal-row-prompt? terminal-key terminal-paste terminal-mouse? terminal-pointer terminal-focus)
  (import (techne) (only (srfi 115) rx regexp-matches? regexp-search regexp-match-submatch))
  (include "terminal/screen.scm" "terminal/reflow.scm" "terminal/vt.scm" "terminal/parse.scm" "terminal/input.scm"))
