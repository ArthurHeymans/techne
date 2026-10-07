;;; Seeing and setting options (modes.scm): describe-option shows an
;;; option's documentation, type and value in the focused buffer, and
;;; every setting that applies there by its cell, the winning one first,
;;; with who made it; set-option sets one in the focused buffer, or
;;; globally with C-u.

(require "session.scm")
(require "modes.scm")
(require "commands.scm")
(require "minibuffer.scm")
(require "lens.scm")

(provide describe-option set-option)

(define (written v) (call-with-output-string (lambda (p) (write v p))))

(define (first-line text)
  (let ((i (string-index text #\newline))) (if i (substring text 0 i) text)))

;; Read an option's name, then (ACCEPT session name).
(define (read-option s prompt accept)
  (completing-read s prompt
                   (map (lambda (name)
                          (candidate (symbol->string name) #:annotation (first-line (option-doc (find-option name)))))
                        (sort (option-names) (lambda (a b) (string<? (symbol->string a) (symbol->string b)))))
                   #:accept (lambda (s c) (accept s (string->symbol (candidate-text c))))))

(define (option-rows b name)
  (let ((o (find-option name)))
    (append (list (row "option" (symbol->string name))
                  (row "value" (written (option b name)))
                  (row "type" (written (option-type o))))
            (map (lambda (l) (row "doc" l)) (string-split (option-doc o) "\n"))
            (map (lambda (e)
                   (row (symbol->string (car e))
                        (string-append (written (cadr e))
                                       (if (eq? (car e) 'default) "" (string-append "  set by " (symbol->string (caddr e)))))))
                 (explain-option b name)))))

(define-command (describe-option s n)
  "Show an option: its documentation, type and value in this buffer, and
the settings that apply here, the one that wins first."
  (let ((b (current-buffer s)))
    (read-option s "Describe option: "
                 (lambda (s name)
                   (show-view! s (string-append "*option " (symbol->string name) "*") (lambda (s) (option-rows b name)))))))

;; The values to offer for TYPE.
(define (type-values type)
  (cond ((eq? type 'boolean) '(#t #f))
        ((and (pair? type) (eq? (car type) 'one-of)) (cdr type))
        ((and (pair? type) (eq? (car type) 'or)) (append-map type-values (cdr type)))
        (else '())))

(define-command (set-option s n)
  "Set an option in this buffer; with C-u, globally."
  (let ((b (current-buffer s)) (global (current-prefix s)))
    (read-option s "Set option: "
                 (lambda (s name)
                   (completing-read s (string-append "Set " (symbol->string name) (if global " globally" "") " to: ")
                                    (map written (type-values (option-type (find-option name))))
                                    #:require-match #f
                                    #:accept (lambda (s c)
                                               (let ((value (read (open-input-string (candidate-text c)))))
                                                 (if global (set-option! name value) (set-option! name value #:buffer b))
                                                 (message! s (string-append (symbol->string name) " is " (written value))))))))))
