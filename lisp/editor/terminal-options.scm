;;; Configuration is available before the terminal implementation is loaded.
(require "modes.scm")

(define-option terminal-editor-keys
  '("C-c" "C-x" "C-\\" "C-g" "C-h" "C-u" "C-M-c" "M-x" "M-:" "M-!" "M-&")
  "Keys left to the editor profile in terminal semi-char mode."
  #:type '(list-of string))

(define-option terminal-detect-passwords #f
  "Whether to offer hidden input when the child disables PTY echo.
A canonical non-echoing read is not always a password prompt."
  #:type 'boolean)

(define-option terminal-close-on-exit #t
  "Whether to close an interactive terminal after its shell exits zero.
Buffers from the terminal-command prompt and unsuccessful exits stay open."
  #:type 'boolean)

(define-option terminal-serial-baud 115200
  "Baud rate for a serial terminal opened with socat."
  #:type '(one-of 9600 19200 38400 57600 115200 230400))

