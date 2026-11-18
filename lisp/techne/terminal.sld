(define-library (techne terminal)
  (export make-terminal terminal-feed! terminal-resize! terminal-reset!
          terminal-rows terminal-columns terminal-row terminal-x terminal-y
          terminal-mode? terminal-title terminal-alternate?
          terminal-history-start terminal-history-end terminal-history-row
          terminal-row-cells terminal-row-cache terminal-row-cache-set! terminal-row-double-width?
          terminal-key terminal-paste)
  (import (techne))
  (include "terminal/screen.scm" "terminal/vt.scm" "terminal/parse.scm" "terminal/input.scm"))
