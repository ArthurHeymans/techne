;;; Text-hit ownership uses the shared definition parser, never a disk reread.
(define (%innermost-definition definitions point)
  (let loop ((rest definitions) (best #f))
    (if (null? rest) best
        (let* ((d (car rest)) (start (caddr d)) (end (cadddr d)))
          (loop (cdr rest)
                (if (and (<= start point) (< point end)
                         (or (not best) (< (- end start) (- (cadddr best) (caddr best)))))
                    d best))))))
(define (%definition-language? path)
  (let ((lang (source-language path)))
    (and lang (or (eq? (language-value lang 'grammar) 'scheme)
                  (language-value lang 'definitions)))))
(define (%search-enclosures p items automatic)
  (let ((cache '()))
    (map
      (lambda (item)
        (if (and automatic (not (%definition-language? (tool-ref item 'path)))) item
        (let* ((found (%snapshot-at p (tool-ref item 'handle) "tool-search"))
               (s (car found)) (point (+ (cadr found) (- (tool-ref item 'column) 1)))
               (entry (assq s cache)))
          (unless entry
            (set! entry
              (cons s
                (guard (e (#t `((definitions ()) (complete #f)
                                (error ,(%clip (condition/report-string e) 240)))))
                  (append (%snapshot-definitions s) `((starts ,(%line-starts (%source s))))))))
            (set! cache (cons entry cache)))
          (let* ((parsed (cdr entry))
                 (owner (%innermost-definition (tool-ref parsed 'definitions) point))
                 (complete (tool-ref parsed 'complete))
                 (status (cond ((tool-ref parsed 'error) 'unavailable)
                               ((not complete) 'partial)
                               (owner 'found) (else 'none))))
            (append item
              `((enclosing ,(and owner (%definition-item p s owner (tool-ref parsed 'starts)
                                                         (tool-ref parsed 'provider))))
                (enclosing-complete ,complete) (enclosing-status ,status))
              (if (tool-ref parsed 'error) `((enclosing-error ,(tool-ref parsed 'error))) '()))))))
      items)))
(define (%show-enclosing item)
  (let ((owner (tool-ref item 'enclosing)) (status (tool-ref item 'enclosing-status)))
    (cond
      (owner
        (string-append "\n[enclosing " (%clip (tool-ref owner 'name) 120)
                       " (" (tool-ref owner 'kind) ", " (tool-ref owner 'scope) "); "
                       (tool-ref owner 'path) ":" (%str (tool-ref owner 'line)) "-"
                       (%str (tool-ref owner 'end-line))
                       (if (eq? status 'partial) "; partial syntax, candidate only" "")
                       "; (tool-expand project " (%written (tool-ref owner 'handle)) ") ]"))
      ((eq? status 'none) "\n[no enclosing named definition]")
      (else (string-append "\n[enclosing definition " (%str status)
                           "; text match remains valid"
                           (if (tool-ref item 'enclosing-error)
                               (string-append ": " (tool-ref item 'enclosing-error)) "") "]")))))
