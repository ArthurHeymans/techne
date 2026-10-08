;;; itl, Interactive Techne Lisp (M-x itl), named as Emacs's ielm is for
;;; Interactive Emacs Lisp Mode: a REPL in a buffer whose transcript (prompts,
;;; inputs, output, results) is generated text and whose input, after the
;;; last prompt, is edited as any text. RET evaluates the input when it is
;;; complete, in the module of the buffer the REPL was opened from, and
;;; puts it in the transcript with what it printed and its value; an
;;; incomplete one gets a new line. M-p and M-n bring back earlier inputs;
;;; TAB completes names, as the popup does by itself (completion.scm).
;;;
;;; It is a presentation (EDITOR.md, section 2): the transcript is rows of
;;; its own, the input an excerpt of a document of its own, after the last
;;; prompt. C-c M-i inspects the last value.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")
(require "buffers.scm")
(require "views.scm")
(require "completion.scm")

(provide itl)

(define prompt "techne> ")

(define-mode itl-mode
  "Interactive Techne Lisp: RET evaluates the input after the prompt, M-p
and M-n bring back earlier ones; TAB completes the name before point."
  #:complete (lambda (b pos)
               (let ((r (buffer-state b)))
                 (and (repl? r) (let ((span (input-span r))) (<= (car span) pos (cadr span)))
                      (scheme-completion (buffer-document b) pos (repl-module r)))))
  #:keys '(("RET" repl-return) ("M-p" repl-previous-input) ("M-n" repl-next-input) ("C-a" repl-beginning-of-line)
           ;; As ielm's TAB.
           ("TAB" completion-at-point) ("C-M-i" completion-at-point)
           ("C-c M-i" inspect-last-result)))

;; The REPL of a buffer: its presentation, the banner, the transcript (a
;; list of (input . output), newest first), the input document, a view
;; writing it, its module and its history.
(define-record-type repl
  (make-repl presentation banner input writer module history at)
  repl?
  (presentation repl-presentation)
  (banner repl-banner)
  (transcript repl-transcript set-repl-transcript!)
  (input repl-input)
  (writer repl-writer)
  (module repl-module)
  (history repl-history set-repl-history!)
  ;; Which history entry M-p and M-n are at.
  (at repl-at set-repl-at!))

(define (repl-of s)
  (let ((b (session-buffer s)))
    (or (and b (repl? (buffer-state b)) (buffer-state b)) (error "Not a REPL"))))

;; The rows: the banner, each input with its prompt and what it gave, then
;; the prompt and the input.
(define (show-rows! r)
  (let ((in (repl-input r)) (p (list prompt 'keyword)))
    (present! (repl-presentation r)
              (append (list (row (repl-banner r) #:key 'banner))
                      (let ((entries (reverse (repl-transcript r))))
                        (append-map (lambda (i)
                                      (let ((e (list-ref entries i)))
                                        (cons (row (list p (car e)) #:key (list 'in i))
                                              (if (string=? (cdr e) "") '() (list (row (cdr e) #:key (list 'out i)))))))
                                    (iota (length entries))))
                      (list (row (list p (excerpt in 0 (document-length in))) #:key 'input))))))

;; The input's span in the REPL's text: after the last prompt.
(define (input-span r)
  (let ((span (presentation-row-span (repl-presentation r) "input")))
    (list (+ (car span) (string-length prompt)) (cadr span))))

(define (set-input! r text)
  (let ((in (repl-input r)))
    (view-edit! (repl-writer r) (list (list 0 (document-length in) text)) "new")
    (show-rows! r)))

(define (to-end! s)
  (let ((end (document-length (doc s))))
    (view-set-ranges! (session-view s) (list (list end end)) 0)))

(define-command (itl s n)
  "Open itl, Interactive Techne Lisp: a REPL evaluating in the module of
the focused buffer's file."
  (let* ((module (document-module (doc s)))
         (input (make-document ""))
         (p (make-presentation))
         (r (make-repl p (string-append ";; Interactive Techne Lisp, evaluating in " module)
                       input (make-view input "repl") module '() #f)))
    (set-repl-transcript! r '())
    (show-rows! r)
    (show-buffer! s (make-generated-buffer! "*itl*" p 'itl-mode #:state r))
    (to-end! s)))

;; Whether the input reads to its end: else RET adds a line.
(define (complete? text)
  (guard (e ((and (error-object? e) (string-contains (error-object-message e) "unexpected end of input")) #f)
            (#t #t))
    (let ((p (open-input-string text)))
      (let loop () (unless (eof-object? (read p)) (loop))))
    #t))

;; What evaluating TEXT printed, then its value written (none when it
;; has none, as for a definition) or the error; the value is kept for
;; C-c M-i.
(define (evaluate s r text)
  (let* ((value #f)
         (output (guard (e (#t (set! value (list 'error e)) ""))
                   (with-output-to-string (lambda () (set! value (list 'ok (eval-source text (repl-module r) "*itl*"))))))))
    (string-append output
                   (if (and (> (string-length output) 0) (not (string-suffix? "\n" output))) "\n" "")
                   (if (eq? (car value) 'ok)
                       (begin (sset! s 'last-result (cadr value))
                              (if (eq? (cadr value) (if #f #f))
                                  ""
                                  (call-with-output-string (lambda (p) (write (cadr value) p)))))
                       (string-append "error: " (error-text (cadr value))))
                   (if (and (eq? (car value) 'ok) (eq? (cadr value) (if #f #f))) "" "\n"))))

(define-command (repl-return s n)
  "Evaluate the input when it is complete, else start a new line of it."
  (let* ((r (repl-of s)) (span (input-span r)) (p (point s)))
    (cond ((or (< p (car span)) (> p (cadr span))) (to-end! s))
          ((not (complete? (document-string (repl-input r)))) (insert-text! s "\n" 'new))
          ((string=? (string-trim (document-string (repl-input r))) "") (to-end! s))
          (else
           (let* ((text (document-string (repl-input r)))
                  (result (evaluate s r text)))
             (set-repl-history! r (cons text (repl-history r)))
             (set-repl-at! r #f)
             (set-repl-transcript! r (cons (cons text (if (string-suffix? "\n" result) (substring result 0 (- (string-length result) 1)) result))
                                           (repl-transcript r)))
             (set-input! r "")
             (to-end! s))))))

(define (recall! s step)
  (let* ((r (repl-of s)) (h (repl-history r)) (n (length h))
         (i (+ (or (repl-at r) -1) step)))
    (cond ((null? h) (message! s "No history"))
          ((< i 0) (set-repl-at! r #f) (set-input! r "") (to-end! s))
          ((>= i n) (message! s "No earlier input"))
          (else (set-repl-at! r i) (set-input! r (list-ref h i)) (to-end! s)))))

(define-command (repl-previous-input s n) "Bring back the input before." (recall! s n))
(define-command (repl-next-input s n) "Bring back the input after." (recall! s (- n)))

(define-command (repl-beginning-of-line s n)
  "Move to the start of the input on the prompt's line, else of the line."
  (let* ((r (repl-of s)) (start (car (input-span r))) (p (point s)) (d (doc s)))
    (move! s (lambda (p) (if (and (>= p start) (= (line-start d p) (line-start d start))) start (line-start d p))))))
