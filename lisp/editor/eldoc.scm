;;; Eldoc, as Emacs's: what is known of the thing at point, said in the
;;; echo area when point rests there — a diagnostic, a call's signature,
;;; a name's type — and the whole of it in *eldoc* on C-h . in the
;;; buffers whose providers know more (K and SPC c k in the modal
;;; profile).
;;;
;;; A provider says what it knows of a position; one that must wait (a
;;; language server's hover) runs in a task, after a pause in typing, and
;;; what it says is dropped if point has moved on. The echo area is only
;;; written when it holds nothing else: a message stays until the next
;;; command clears it.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "buffers.scm")
(require "requests.scm")

(provide define-eldoc-provider register-eldoc-provider! eldoc-provider-names eldoc-idle-delay eldoc-doc-buffer)

(define eldoc-idle-delay 300
  "The milliseconds point rests before eldoc says what is there.")

(define-record-type eldoc-provider
  (%make-provider name doc applies say priority)
  eldoc-provider?
  (name provider-name)
  (doc provider-doc)
  ;; (buffer) -> whether it knows of this buffer.
  (applies provider-applies)
  ;; (session document position full?) -> a string or #f; it may wait.
  (say provider-say)
  (priority provider-priority))

(define %providers (make-registry 'eldoc-providers))

(define (register-eldoc-provider! name doc #:applies [applies (lambda (b) #t)] #:say say #:priority [priority 0])
  "Define the eldoc provider NAME, documented by DOC.
APPLIES is a procedure (buffer) saying whether it knows of the buffer;
SAY (session document position full?) returns what it knows of
POSITION — a line for the echo area, all of it with FULL? — or #f. It
runs in a task, and may wait. Providers are asked in order of
PRIORITY, highest first: the echo area says the first answer, *eldoc*
all of them."
  (registry-add! %providers name (%make-provider name doc (scope-procedure applies) (scope-procedure say) priority))
  name)

(define-syntax define-eldoc-provider
  (syntax-rules ()
    "Define the eldoc provider NAME, documented by DOC.
The other arguments are keywords, as `register-eldoc-provider!` takes them."
    ((_ name doc arg ...) (register-eldoc-provider! 'name doc arg ...))))

(define (eldoc-provider-names)
  "Return the names of the eldoc providers defined."
  (registry-keys %providers))

(define (providers-for b)
  (sort (filter (lambda (p) (and b ((provider-applies p) b)))
                (filter-map (lambda (n) (registry-ref %providers n)) (registry-keys %providers)))
        (lambda (a b) (> (provider-priority a) (provider-priority b)))))

;; The first line of TEXT that says something.
(define (first-line text)
  (let ((lines (filter (lambda (l) (not (string=? (string-trim l) ""))) (string-split text "\n"))))
    (if (null? lines) #f (string-trim (car lines)))))

;;; The echo area, as point rests.

;; A request slot per session.
(define (eldoc-slot s)
  (or (sget s 'eldoc-slot) (let ((slot (make-request-slot))) (sset! s 'eldoc-slot slot) slot)))

;; Where eldoc last spoke: (document revision position), so a key that
;; leaves point where it was asks nothing again.
(define (eldoc-after-key s)
  (let ((b (current-buffer s)))
    (when (and b (not (sget s 'minibuffer)))
      (let* ((d (doc s)) (pos (point s)) (here (list (document-id d) (document-revision d) pos))
             (providers (providers-for b)))
        (cond ((null? providers) (cancel-request! (eldoc-slot s)))
              ((equal? here (sget s 'eldoc-at))
               ;; Asked here already: what was said is said again if a
               ;; command cleared it (C-a at the line's start).
               (let ((said (sget s 'eldoc-said)))
                 (when (and said (not (sget s 'message))) (sset! s 'message said))))
              (else
               (sset! s 'eldoc-at here)
               (sset! s 'eldoc-said #f)
               (request! (eldoc-slot s)
                         (lambda ()
                           (sleep eldoc-idle-delay)
                           (let ask ((ps providers))
                             (and (pair? ps)
                                  (let ((text (guard (e (#t #f)) ((provider-say (car ps)) s d pos #f))))
                                    (or (and (string? text) (first-line text)) (ask (cdr ps)))))))
                         (lambda (text)
                           ;; Still there, and nothing else being said.
                           (when (and text (equal? (sget s 'eldoc-at) here)
                                      (equal? (list (document-id (doc s)) (document-revision (doc s)) (point s)) here)
                                      (not (sget s 'message)) (not (sget s 'minibuffer)))
                             (sset! s 'eldoc-said text)
                             (sset! s 'message text))))))))))

(add-hook! 'after-key 'eldoc (lambda (s key) (eldoc-after-key s)))

;;; All of it, in a buffer.

(define-command (eldoc-doc-buffer s n)
  "Show in *eldoc* all the providers know of the thing at point.
As the echo area says it, at length: a name's documentation, the
diagnostics there."
  (let* ((b (or (current-buffer s) (error "No buffer"))) (d (doc s)) (pos (point s)) (providers (providers-for b)))
    (when (null? providers) (error "Nothing here says what is at point"))
    (message! s "Asking…")
    (request! (eldoc-slot s)
              (lambda ()
                (filter-map (lambda (p) (let ((text (guard (e (#t #f)) ((provider-say p) s d pos #t)))) (and (string? text) text)))
                            providers))
              (lambda (texts)
                (if (null? texts)
                    (message! s "Nothing known of what is at point")
                    (let ((b (make-generated-buffer! "*eldoc*" (make-document (string-join texts "\n\n")) 'special-mode)))
                      (set-option! 'syntax-language 'markdown #:buffer b)
                      (message! s #f)
                      (display-buffer! s (buffer-document b)))))
              #:fail (lambda (e) (message! s (error-text e))))))
