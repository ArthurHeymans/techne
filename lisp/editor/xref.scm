;;; Cross-references, as Emacs's xref: the definitions of the thing at
;;; point, its references, implementations and type, and the names of a
;;; project, from whichever backend knows them here. M-. goes to a
;;; definition, M-? (and C-. where it can be typed) lists the
;;; references, M-, comes back; C-M-. finds a name in the whole project.
;;;
;;; A backend says where it applies and finds the kinds it knows: a
;;; language server (lsp.scm), the live Scheme module (live.scm). One
;;; that must wait (for a server's answer) runs in a task, so the editor
;;; goes on meanwhile; one that need not answers at once. One thing found
;;; is gone to; several are listed with a preview (consult-xref), C-g
;;; putting the panes back. Going somewhere remembers where it left, for
;;; M-, to come back to, however far, in the same file too.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "requests.scm")
(require "files.scm")

(provide define-xref-backend register-xref-backend! xref-backend-names xref-item xref-item? xref-item-location xref-item-summary
         xref-find-definitions xref-find-references xref-find-implementations xref-find-type-definition xref-find-apropos
         xref-push-marker! xref-go-back)

;;; Items: a place, and what to say of it.

(define-record-type xref-item
  (%xref-item location summary group)
  xref-item?
  (location xref-item-location)
  (summary xref-item-summary)
  (group xref-item-group))

(define (xref-item l #:summary [summary #f] #:group [group #f])
  "Return an xref item for the location L.
SUMMARY is what the list says of it, its line unless given; GROUP what
it is listed under (its file unless given)."
  (%xref-item l summary group))

;;; Backends

(define-record-type xref-backend
  (%make-backend name doc applies find wait priority)
  xref-backend?
  (name backend-name)
  (doc backend-doc)
  ;; (session buffer) -> whether it knows of this buffer.
  (applies backend-applies)
  ;; (session kind document position pattern) -> items, or #f for "not
  ;; mine": KIND is definitions, references, implementations,
  ;; type-definitions or symbols (PATTERN then the name typed).
  (find backend-find)
  ;; Whether finding waits: then it runs in a task.
  (wait backend-wait)
  ;; Higher first.
  (priority backend-priority))

(define %backends (make-registry 'xref-backends))

(define (register-xref-backend! name doc #:applies [applies (lambda (s b) #t)] #:find find #:wait [wait #f] #:priority [priority 0])
  "Define the xref backend NAME, documented by DOC.
APPLIES is a procedure (session buffer) saying whether it knows of the
buffer; FIND (session kind document position pattern) returns the
`xref-item`s of KIND at POSITION of DOCUMENT — definitions,
references, implementations, type-definitions; or symbols, matching
PATTERN — or #f when it knows nothing of that kind. With WAIT, FIND
waits (on a server) and runs in a task. Backends are asked in order of
PRIORITY, highest first, until one finds something."
  (registry-add! %backends name (%make-backend name doc (scope-procedure applies) (scope-procedure find) wait priority))
  name)

(define-syntax define-xref-backend
  (syntax-rules ()
    "Define the xref backend NAME, documented by DOC.
The other arguments are keywords, as `register-xref-backend!` takes them."
    ((_ name doc arg ...) (register-xref-backend! 'name doc arg ...))))

(define (xref-backend-names)
  "Return the names of the xref backends defined."
  (registry-keys %backends))

(define (backends-for s b)
  (sort (filter (lambda (x) (and b ((backend-applies x) s b)))
                (filter-map (lambda (n) (registry-ref %backends n)) (registry-keys %backends)))
        (lambda (a b) (> (backend-priority a) (backend-priority b)))))

;;; Going and coming back

(define (xref-push-marker! s)
  "Remember where session S's focused pane is, for \\[xref-go-back]."
  (let ((v (pane-view s)))
    (sset! s 'visited (cons (cons v (location (view-document v) (view-point v))) (or (sget s 'visited) '())))))

(define-command (xref-go-back s n)
  "Go back to where the last jump to a definition or reference left.
The pane shows that buffer again, at that place, however it was edited."
  (let ((visited (sget s 'visited)))
    (if (null? (or visited '()))
        (message! s "Nothing to go back to")
        (let* ((entry (car visited))
               (v (if (pair? entry) (car entry) entry))
               (b (document-buffer (view-document v))))
          (sset! s 'visited (cdr visited))
          (set-pane-view! s v)
          (when (pair? entry)
            (let ((p (guard (e (#t #f)) (location-position (cdr entry)))))
              (when p (view-set-ranges! v (list (list p p)) 0))))
          ;; Back where it was, it is the buffer shown last, for C-x b.
          (when b (remember-buffer! b))))))

(define (go! s item)
  (xref-push-marker! s)
  (act-default! s (target 'location (xref-item-location item))))

;;; Finding

;; A request slot per session: one session's asking cancels its own
;; asking before, not another frontend's.
(define (xref-slot s)
  (or (sget s 'xref-slot) (let ((slot (make-request-slot))) (sset! s 'xref-slot slot) slot)))

(define (kind-label kind)
  (case kind
    ((definitions) "definitions") ((references) "references") ((implementations) "implementations")
    ((type-definitions) "type definitions") (else "symbols")))

;; What a backend found, where it was asked from: one is gone to, several
;; listed.
(define (show-items! s kind items)
  (cond ((null? items) (message! s (string-append "No " (kind-label kind) " found")))
        ((and (null? (cdr items)) (not (eq? kind 'references))) (go! s (car items)))
        (else (choose! s kind items))))

(define (capitalized text)
  (if (string=? text "") text (string-append (string (char-upcase (string-ref text 0))) (substring text 1 (string-length text)))))

(define (summary-of item)
  (or (xref-item-summary item)
      (let ((l (xref-item-location item)))
        (guard (e (#t ""))
          (let ((d (location-document l)))
            (string-trim (line-candidate-text d (location-position l))))))))

(define (where-of item)
  (let* ((l (xref-item-location item)) (d (location-document l)) (p (location-position l)))
    (string-append (or (xref-item-group item) (let ((path (document-path d))) (if path (abbreviate-path path) "")))
                   ":" (number->string (line-number d p)))))

(define (choose! s kind items)
  (let ((origin (pane-view s)) (at (point s)))
    (completing-read s (string-append (capitalized (kind-label kind)) ": ")
                     (map (lambda (item)
                            (candidate (summary-of item) #:annotation (where-of item)
                                       #:target (target 'location (xref-item-location item))))
                          items)
                     #:preview (lambda (s c) (act-default! s (candidate-target c)))
                     #:accept (lambda (s c)
                                ;; Where it was before the previews, to come back to.
                                (sset! s 'visited (cons (cons origin (location (view-document origin) at)) (or (sget s 'visited) '())))
                                (act-default! s (candidate-target c))))))

(define (find! s kind)
  (let* ((b (current-buffer s)) (d (doc s)) (pos (point s)) (backends (backends-for s b))
         (origin (pane-view s)))
    (when (null? backends) (error (string-append "No backend here finds " (kind-label kind))))
    ;; A newer question: the answer to an older one is not wanted.
    (cancel-request! (xref-slot s))
    ;; In order of priority: those that answer at once are asked here;
    ;; from the first that must wait on, in a task, and then, back here,
    ;; the ones that answer at once after it. Delivered only while the
    ;; pane is where it was asked from.
    (let loop ((bs backends))
      (cond ((null? bs) (message! s (string-append "No " (kind-label kind) " found")))
            ((backend-wait (car bs))
             (message! s (string-append "Looking for " (kind-label kind) "…"))
             (let ((waiting (let take ((bs bs)) (if (and (pair? bs) (backend-wait (car bs))) (cons (car bs) (take (cdr bs))) '())))
                   (after (let drop ((bs bs)) (if (and (pair? bs) (backend-wait (car bs))) (drop (cdr bs)) bs))))
               (request! (xref-slot s)
                         (lambda ()
                           (let ask ((bs waiting))
                             (if (null? bs)
                                 '()
                                 (let ((found ((backend-find (car bs)) s kind d pos #f)))
                                   (if (and found (pair? found)) found (ask (cdr bs)))))))
                         (lambda (items)
                           (message! s #f)
                           (when (and (eq? (pane-view s) origin) (= (point s) pos))
                             (guard (e (#t (message! s (error-text e))))
                               (if (pair? items) (show-items! s kind items) (loop after)))))
                         #:fail (lambda (e) (message! s (error-text e))))))
            (else
             (let ((found ((backend-find (car bs)) s kind d pos #f)))
               (if (and found (pair? found)) (show-items! s kind found) (loop (cdr bs)))))))))

(define-command (xref-find-definitions s n)
  "Go to the definition of the thing at point.
Where there are several, they are listed to choose from, each previewed.
\\[xref-go-back] comes back."
  (find! s 'definitions))

(define-command (xref-find-references s n)
  "List the references to the thing at point, previewing each.
\\[xref-go-back] comes back from the one chosen."
  (find! s 'references))

(define-command (xref-find-implementations s n)
  "Go to the implementations of the thing at point."
  (find! s 'implementations))

(define-command (xref-find-type-definition s n)
  "Go to the definition of the type of the thing at point."
  (find! s 'type-definitions))

(define-command (xref-find-apropos s n)
  "Find a name in the project as it is typed, and go to it.
The candidates are the backend's matches for what is typed so far."
  (let* ((b (current-buffer s)) (d (doc s)) (pos (point s))
         (backends (filter backend-wait (backends-for s b))))
    (when (null? backends) (error "No backend here finds names"))
    (completing-read s "Find symbol: "
                     (async-source (lambda (input)
                                     (let ask ((bs backends))
                                       (if (null? bs)
                                           '()
                                           (let ((found ((backend-find (car bs)) s 'symbols d pos input)))
                                             (if (and found (pair? found))
                                                 (map (lambda (item)
                                                        (candidate (summary-of item) #:annotation (where-of item)
                                                                   #:target (target 'location (xref-item-location item))))
                                                      found)
                                                 (ask (cdr bs)))))))
                                   #:min-input 1)
                     #:preview (lambda (s c) (act-default! s (candidate-target c)))
                     #:accept (let ((origin (pane-view s)) (at pos))
                                (lambda (s c)
                                  (sset! s 'visited (cons (cons origin (location (view-document origin) at)) (or (sget s 'visited) '())))
                                  (act-default! s (candidate-target c)))))))
