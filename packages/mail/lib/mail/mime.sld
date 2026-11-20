;;; MIME parsing is Scheme policy; the generic base64 codec is shared.
(define-library (mail mime)
  (export mail-parse mail-header mail-header-raw mail-decode-header mail-decode-utf8 mail-text mail-html mail-attachments)
  (import (techne) (techne crypto) (techne task) (mail charset))
  (include "mime.scm"))
