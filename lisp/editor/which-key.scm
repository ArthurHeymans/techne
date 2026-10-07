;;; which-key: after a prefix key (C-x, C-c, SPC...) and a pause, the keys
;;; that can follow it, with what they do, are shown below the panes; once
;;; shown, a further prefix shows its own at once. The reference is
;;; Arthur's Emacs: a delay of a second, keys in alphabetical order, a
;;; prefix shown as +its name, descriptions cut at 27 characters.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "emacs.scm")
(require "modal.scm")
(require "minibuffer.scm")

(provide editor-key-hints which-key-idle-delay)

;; Milliseconds.
(define which-key-idle-delay 1000)
(define which-key-max-description-length 27)

;; The prefix being typed, and the keymaps it is looked up in.
(define (typed-prefix s)
  (cond ((sget s 'minibuffer) (values (or (sget s 'mb-pending) '()) (list minibuffer-map)))
        ((eq? (profile-name (sget s 'profile)) 'modal)
         (let ((keys (or (sget s 'mode-pending) '())))
           (values keys (if (null? keys) '() (modal-keymaps s keys)))))
        (else (values (or (sget s 'pending) '()) (active-keymaps s 'chord)))))

;; After every key: the keys shown follow the prefix; a new prefix shows
;; them after the delay, unless another key comes first.
(add-hook! 'after-key 'which-key (lambda (s key) (which-key-after-key s)))

(define (which-key-after-key s)
  (call-with-values (lambda () (typed-prefix s))
    (lambda (keys maps)
      (let ((token (+ 1 (or (sget s 'which-key-token) 0))))
        (sset! s 'which-key-token token)
        (cond ((null? keys) (sset! s 'which-key #f))
              ((sget s 'which-key) (sset! s 'which-key keys))
              (else
               (spawn (lambda ()
                        (sleep which-key-idle-delay)
                        (when (eqv? (sget s 'which-key-token) token)
                          (sset! s 'which-key keys))))))))))

(define (modifiers key)
  (let loop ((k key) (n 0))
    (if (and (> (string-length k) 2) (char=? (string-ref k 1) #\-) (memv (string-ref k 0) '(#\C #\M #\s #\S #\H)))
        (loop (substring k 2 (string-length k)) (+ n 1))
        n)))

;; Keys without modifiers first, then alphabetically, ignoring case.
(define (key<? a b)
  (let ((ma (modifiers a)) (mb (modifiers b)))
    (if (= ma mb)
        (let ((la (string-downcase a)) (lb (string-downcase b)))
          (if (string=? la lb) (string<? a b) (string<? la lb)))
        (< ma mb))))

(define (cut text)
  (if (> (string-length text) which-key-max-description-length)
      (string-append (substring text 0 (- which-key-max-description-length 1)) "…")
      text))

;; What the frontend shows: (key description prefix?) for each key that
;; can follow the prefix, while they are shown.
(define (editor-key-hints s)
  (let ((shown (sget s 'which-key)))
    (if (not shown)
        '()
        (call-with-values (lambda () (typed-prefix s))
          (lambda (keys maps)
            (map (lambda (b)
                   (let ((binding (cdr b)))
                     (if (keymap? binding)
                         (list (car b) (string-append "+" (or (keymap-name binding) "prefix")) #t)
                         (list (car b) (cut (symbol->string binding)) #f))))
                 (sort (filter cdr (prefix-bindings maps keys)) (lambda (a b) (key<? (car a) (car b))))))))))
