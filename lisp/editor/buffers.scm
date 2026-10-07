;;; Buffers (EDITOR.md, section 1): what can be switched to (modes.scm).
;;; A file's buffer is named by its file and gets the major mode for its
;;; name; a document of no file is *scratch*, in scheme-mode. Each buffer
;;; remembers the view it was last shown in, so going back to it finds its
;;; caret and scroll where they were.
;;;
;;; Files, buffers and locations are targets, with their actions.

(require "session.scm")
(require "keymaps.scm")
(require "dispatch.scm")
(require "modes.scm")
(require "commands.scm")
(require "targets.scm")
(require "files.scm")
(require "minibuffer.scm")

(provide add-buffer! show-document! show-buffer! visit! default-directory
         show-in-other-pane! display-buffer! document-module read-file-name find-file switch-to-buffer kill-buffer line-candidate
         search-lines search-all-buffers make-generated-buffer! buffer-named)

;;; Making buffers

(define (buffer-named name) (find (lambda (b) (equal? (buffer-name b) name)) (buffer-list)))

;; The buffer of D, made if it has none, first in the list. A file's is
;; named by the file, with its directory's name after it when that name is
;; taken.
(define (add-buffer! d)
  (let ((b (or (document-buffer d)
               (let ((path (document-path d)))
                 (when path (file-document path #:document d))
                 (make-buffer d
                              (cond ((not path) "*scratch*")
                                    ((buffer-named (file-name path))
                                     (let ((dir (directory-of (absolute-path path))))
                                       (string-append (file-name path) "<" (file-name (substring dir 0 (- (string-length dir) 1))) ">")))
                                    (else (file-name path)))
                              (if path (mode-for-file path) 'scheme-mode))))))
    (remember-buffer! b)
    b))

;; A buffer NAME of D (a document or a presentation) in MODE, with STATE,
;; replacing a buffer of that name.
(define (make-generated-buffer! name d mode #:state [state #f])
  (let ((old (buffer-named name)))
    (when old (forget-buffer! old))
    (let ((b (make-buffer d name mode #:state state)))
      (remember-buffer! b)
      b)))

(define (new-view b) (make-view (buffer-document b) "user"))

;;; Showing buffers

;; Show D in the focused pane (see show-buffer!).
(define (show-document! s d #:remember [remember #t])
  (show-buffer! s (or (document-buffer d) (add-buffer! d)) #:remember remember))

;; Show B in the focused pane, in the view it was last shown in unless
;; another pane shows that one, read-only if its option says. With
;; REMEMBER false (a preview) the list's order stays.
(define (show-buffer! s b #:remember [remember #t])
  (let ((leaving (pane-view s)))
    (if (document=? (view-document leaving) (buffer-document b))
        leaving
        (let* ((last (buffer-view b))
               (v (cond ((not last) (new-view b))
                        ((any (lambda (p) (view=? p last)) (session-panes s)) (view-split last))
                        (else last)))
               (left (document-buffer (view-document leaving))))
          (when left (set-buffer-view! left leaving))
          (set-view-read-only! v (option b 'read-only))
          (set-buffer-view! b v)
          (when remember (remember-buffer! b))
          (set-pane-view! s v)
          v))))

;; Show line LINE (1-based), column COLUMN, of the file PATH in the focused
;; pane, remembering where it was (M-, goes back).
(define (visit! s path line column)
  (sset! s 'visited (cons (pane-view s) (or (sget s 'visited) '())))
  (let* ((d (file-document path))
         (v (show-document! s d))
         (p (line-down d 0 (- line 1) (- column 1))))
    (view-set-ranges! v (list (list p p)) 0)
    (view-set-scroll! v (line-start d p))))

;;; Commands

(define (directory-name? path) (string-suffix? "/" path))

;; Read a file name, completing it a directory at a time: the candidates
;; are the entries of the input's directory, matched against what follows
;; it. Taking a directory goes into it; (ACCEPT session path) gets a file's
;; absolute path, also one that does not exist yet.
(define (read-file-name s prompt initial accept)
  (completing-read s prompt
                   (lambda (input)
                     (let ((dir (directory-of input)))
                       (map (lambda (name) (candidate name #:target (target 'file (string-append dir name))))
                            (guard (e (#t '()))
                              (directory-list (if (string=? dir "") (working-directory) (absolute-path dir)))))))
                   #:initial initial
                   #:pattern file-name
                   #:require-match #f
                   #:accept (lambda (s c)
                              (let ((path (if (candidate-target c) (target-value (candidate-target c)) (candidate-text c))))
                                (if (directory-name? path)
                                    (read-file-name s prompt path accept)
                                    (accept s (absolute-path path)))))))

(define (default-directory s)
  (let ((path (document-path (view-document (pane-view s)))))
    (directory-of (absolute-path (or path (string-append (working-directory) "/"))))))

(define-command (find-file s n)
  "Open a file in the focused pane, completing its name."
  (read-file-name s "Find file: " (default-directory s)
                  (lambda (s path) (show-document! s (file-document path)))))

;; A file's buffer is marked modified while it has unsaved edits; other
;; buffers are not saved anywhere.
(define (buffer-annotation b)
  (let ((d (buffer-document b)))
    (string-append (if (and (document-path d) (document-dirty? d)) "modified  " "") (or (document-path d) ""))))

(define-command (switch-to-buffer s n)
  "Show another buffer in the focused pane, previewing it while choosing."
  (let ((current (current-buffer s)))
    (completing-read s "Switch to buffer: "
                     (map (lambda (b) (candidate (buffer-name b) #:annotation (buffer-annotation b) #:target (target 'buffer b)))
                          (append (remove (lambda (b) (eq? b current)) (buffer-list)) (if current (list current) '())))
                     #:preview (lambda (s c) (show-buffer! s (target-value (candidate-target c)) #:remember #f)))))

;; Take B off the buffer list; panes showing it show the next buffer. A
;; file's document stays open, with its unsaved edits.
(define (drop-buffer! s b)
  (let ((rest (remove (lambda (x) (eq? x b)) (buffer-list))) (focus (session-focus s)))
    (when (null? rest) (error "The only buffer"))
    (for-each (lambda (i)
                (when (document=? (view-document (list-ref (session-panes s) i)) (buffer-document b))
                  (sset! s 'focus i)
                  (show-buffer! s (car rest) #:remember #f)))
              (iota (length (session-panes s))))
    (sset! s 'focus focus)
    (forget-buffer! b)))

(define-command (kill-buffer s n)
  "Take the focused buffer off the buffer list. A file's unsaved edits stay
in its journal."
  (drop-buffer! s (or (current-buffer s) (error "No buffer"))))

;; Show D in a new pane below the focused one, and focus it.
(define (show-in-other-pane! s d)
  (let ((i (session-focus s)))
    (split-pane! s 'below (view-split (pane-view s)))
    (sset! s 'focus (+ i 1))
    (show-document! s d)))

;; The module code of D evaluates in: its file's when it is Scheme, else
;; the user module.
(define (document-module d)
  (let ((b (document-buffer d)) (path (document-path d)))
    (if (and path b (derived-mode? (buffer-mode b) 'scheme-mode)) path "user")))

;; Show D in a pane without leaving the focused one: in the pane that shows
;; it already, else in a new one below.
(define (display-buffer! s d)
  (unless (any (lambda (v) (document=? (view-document v) d)) (session-panes s))
    (let ((focus (session-focus s)))
      (show-in-other-pane! s d)
      (sset! s 'focus focus))))

;;; Searching lines

;; A candidate for the line at POS of D: its text, its line number, and
;; its location as the target.
(define (line-candidate d pos #:annotation [annotation ""])
  (candidate (line-candidate-text d pos)
             #:annotation (string-append annotation (if (string=? annotation "") "" ":")
                                         (number->string (line-number d pos)))
             #:target (target 'location (location d (line-start d pos)))))

;; Candidates for the lines of D that are not empty.
(define (line-candidates d #:annotation [annotation ""])
  (filter-map (lambda (l)
                (and (not (string=? (cadr l) ""))
                     (candidate (cadr l)
                                #:annotation (string-append annotation (if (string=? annotation "") "" ":")
                                                            (number->string (caddr l)))
                                #:target (target 'location (location d (car l))))))
              (document-lines d)))

(define (preview-target s c) (act-default! s (candidate-target c)))

(define-command (search-lines s n)
  "Go to a line of this buffer, previewing each line while choosing."
  (completing-read s "Go to line: " (line-candidates (doc s)) #:preview preview-target))

(define-command (search-all-buffers s n)
  "Go to a line of any buffer, previewing each line while choosing."
  (completing-read s "Go to line in buffers: "
                   (append-map (lambda (b) (line-candidates (buffer-document b) #:annotation (buffer-name b))) (buffer-list))
                   #:preview preview-target))

;;; Actions on files, buffers and locations; the first is the default.

(define-action file (visit-file s path) "Open the file." (show-document! s (file-document path)))
(define-action file (visit-file-other-pane s path) "Open the file in a new pane below." (show-in-other-pane! s (file-document path)))
(define-action file (copy-file-name s path) "Save the file's name as a kill." (kill-save! s path #f #f))

(define-action buffer (show-buffer s b) "Show the buffer." (show-buffer! s b))
(define-action buffer (show-buffer-other-pane s b) "Show the buffer in a new pane below." (show-in-other-pane! s (buffer-document b)))
(define-action buffer (save-buffer-document s b) "Write the buffer to its file." (document-save! (buffer-document b)))
(define-action buffer (kill-buffer-target s b) "Take the buffer off the buffer list." (drop-buffer! s b))

;; The focused pane shows the location's document with the caret there.
(define (goto-location! s l show)
  (let* ((p (location-position l)) (v (show s (location-document l))))
    (view-set-ranges! v (list (list p p)) 0)))

(define-action location (goto-location s l) "Go to the location." (goto-location! s l show-document!))
(define-action location (goto-location-other-pane s l) "Go to the location in a new pane below." (goto-location! s l show-in-other-pane!))
(define-action location (copy-location-line s l)
  "Save the location's line as a kill."
  (kill-save! s (line-candidate-text (location-document l) (location-position l)) #f #f))
