;;; Lightweight parameter and JSON-object overlay primitives shared by
;;; `(techne project-settings)` and the editor startup layer without
;;; eagerly loading `.techne.scm` / `.editorconfig` parsers.

(define-library (techne project-settings params)
  (export default-trust-store-path
          json-unset json-unset?
          json-object-strip-unset json-object-overlay json-object-compose-overlays)
  (import (techne))
  (begin

    (define (copy-store-path v)
      (if (string? v) (string-copy v) v))

    (define default-trust-store-path
      (make-parameter #f copy-store-path)
      "Optional file path for persisting approved project-setting digests.
When `#f`, falls back to the editor trust-store hook (if registered), or
keeps trust decisions in memory in headless mode. Set to `'none` to keep
decisions in memory only.")

    (define-record-type %json-unset-type
      (%make-json-unset)
      json-unset?)

    (define json-unset (%make-json-unset)
      "Sentinel value removing a key during `json-object-overlay`.")

    (define (json-object-strip-unset val)
      "Return VAL with any `json-unset` keys recursively removed from dicts."
      (cond
        ((dict? val)
         (let loop ((keys (dict-keys val)) (out (dict)))
           (if (null? keys)
               out
               (let* ((k (car keys))
                      (v (dict-ref val k)))
                 (if (json-unset? v)
                     (loop (cdr keys) out)
                     (loop (cdr keys) (dict-set out k (json-object-strip-unset v))))))))
        (else val)))

    (define (json-object-overlay base overlay)
      "Overlay JSON-object dict OVERLAY onto BASE.
Recurses only when both values are dicts; replaces arrays and scalars
wholesale. Preserves `'null` as JSON null and deletes keys mapped to
`json-unset`."
      (cond
        ((and (dict? base) (dict? overlay))
         (let loop ((keys (dict-keys overlay)) (acc (json-object-strip-unset base)))
           (if (null? keys)
               acc
               (let* ((k (car keys))
                      (ov (dict-ref overlay k)))
                 (cond
                   ((json-unset? ov)
                    (loop (cdr keys) (dict-delete acc k)))
                   ((and (dict-contains? acc k) (dict? (dict-ref acc k)) (dict? ov))
                    (loop (cdr keys) (dict-set acc k (json-object-overlay (dict-ref acc k) ov))))
                   (else
                    (loop (cdr keys) (dict-set acc k (json-object-strip-unset ov)))))))))
        (else (json-object-strip-unset overlay))))

    (define (json-object-compose-overlays base overlay)
      "Compose two JSON-object overlay dicts BASE and OVERLAY.
Like `json-object-overlay`, recurses only when both values are dicts and
replaces arrays/scalars wholesale, but preserves `json-unset` markers so
the resulting overlay can still remove keys from an even lower base."
      (cond
        ((and (dict? base) (dict? overlay))
         (let loop ((keys (dict-keys overlay)) (acc base))
           (if (null? keys)
               acc
               (let* ((k (car keys))
                      (ov (dict-ref overlay k)))
                 (cond
                   ((json-unset? ov)
                    (loop (cdr keys) (dict-set acc k json-unset)))
                   ((and (dict-contains? acc k) (dict? (dict-ref acc k)) (dict? ov))
                    (loop (cdr keys) (dict-set acc k (json-object-compose-overlays (dict-ref acc k) ov))))
                   (else
                    (loop (cdr keys) (dict-set acc k ov))))))))
        (else overlay)))))
