;;; Pure conversion; safe URL strings are bridged to targets by the reader.
(define-library (mail html)
  (export mail-html->rich)
  (import (techne))
  (include "html.scm"))
