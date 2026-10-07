;;; Buffers (EDITOR.md, section 1): what can be switched to. A buffer is a
;;; document with a name; the list keeps them most recently shown first.
;;; Each remembers the view it was last shown in, so going back to it finds
;;; its caret and scroll where they were.
;;;
;;; Files, buffers and locations are targets, with their actions.

(require "session.scm")
(require "commands.scm")
(require "targets.scm")
(require "minibuffer.scm")

(provide buffer-list buffer-name add-buffer! forget-buffer! show-document! visit! default-directory
         show-in-other-pane! display-buffer! document-module read-file-name find-file switch-to-buffer kill-buffer line-candidate
         search-lines search-all-buffers)

;;; The buffer list

(define %buffers '())

(define (buffer-list) %buffers)

(define (buffer-name d) (doc-prop d 'name))

;; Put D first in the list, naming it if it is new: a file by its name,
;; with its directory's name after it when that name is taken.
(define (add-buffer! d)
  (unless (buffer-name d)
    (let* ((path (document-path d))
           (base (if path (file-name path) "*scratch*"))
           (taken? (lambda (n) (any (lambda (b) (equal? (buffer-name b) n)) %buffers))))
      (when path (file-document path #:document d))
      (set-doc-prop! d 'name
                     (if (and path (taken? base))
                         (let ((dir (directory-of (absolute-path path))))
                           (string-append base "<" (file-name (substring dir 0 (- (string-length dir) 1))) ">"))
                         base))))
  (set! %buffers (cons d (remove (lambda (b) (document=? b d)) %buffers))))

;; Take D off the list (not out of the panes).
(define (forget-buffer! d)
  (set! %buffers (remove (lambda (b) (document=? b d)) %buffers)))

;; Show D in the focused pane, in the view it was last shown in unless
;; another pane shows that one. With REMEMBER false (a preview) the list's
;; order stays.
(define (show-document! s d #:remember [remember #t])
  (let ((leaving (pane-view s)))
    (if (document=? (view-document leaving) d)
        leaving
        (let* ((last (doc-prop d 'view))
               (v (cond ((not last) (make-view d "user"))
                        ((any (lambda (p) (view=? p last)) (session-panes s)) (view-split last))
                        (else last))))
          (when (doc-prop d 'read-only) (set-view-read-only! v #t))
          (set-doc-prop! (view-document leaving) 'view leaving)
          (set-doc-prop! d 'view v)
          (if remember (add-buffer! d) (unless (buffer-name d) (add-buffer! d)))
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

(define (buffer-annotation d)
  (string-append (if (document-dirty? d) "modified  " "") (or (document-path d) "")))

(define-command (switch-to-buffer s n)
  "Show another buffer in the focused pane, previewing it while choosing."
  (let ((current (view-document (pane-view s))))
    (completing-read s "Switch to buffer: "
                     (map (lambda (d) (candidate (buffer-name d) #:annotation (buffer-annotation d) #:target (target 'buffer d)))
                          (append (remove (lambda (d) (document=? d current)) (buffer-list)) (list current)))
                     #:preview (lambda (s c) (show-document! s (target-value (candidate-target c)) #:remember #f)))))

;; Take D off the buffer list; panes showing it show the next buffer. A
;; file's document stays open, with its unsaved edits.
(define (drop-buffer! s d)
  (let ((rest (remove (lambda (b) (document=? b d)) %buffers)))
    (when (null? rest) (error "The only buffer"))
    (set! %buffers rest)
    (set-session-panes! s
                        (map (lambda (v) (if (document=? (view-document v) d) (make-view (car rest) "user") v))
                             (session-panes s))
                        (session-focus s))
    (for-each (lambda (v) (set-doc-prop! (car rest) 'view v))
              (filter (lambda (v) (document=? (view-document v) (car rest))) (session-panes s)))))

(define-command (kill-buffer s n)
  "Take the focused buffer off the buffer list. A file's unsaved edits stay
in its journal."
  (drop-buffer! s (view-document (pane-view s))))

;; Show D in a new pane below the focused one, and focus it.
(define (show-in-other-pane! s d)
  (let ((panes (session-panes s)) (i (session-focus s)))
    (set-session-panes! s (append (take panes (+ i 1)) (list (view-split (pane-view s))) (drop panes (+ i 1))) (+ i 1))
    (show-document! s d)))

;; The module code of D evaluates in: its file's, else the user module.
(define (document-module d)
  (let ((path (document-path d)))
    (if (and path (string-suffix? ".scm" path)) path "user")))

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
                   (append-map (lambda (d) (line-candidates d #:annotation (buffer-name d))) (buffer-list))
                   #:preview preview-target))

;;; Actions on files, buffers and locations; the first is the default.

(define-action file (visit-file s path) "Open the file." (show-document! s (file-document path)))
(define-action file (visit-file-other-pane s path) "Open the file in a new pane below." (show-in-other-pane! s (file-document path)))
(define-action file (copy-file-name s path) "Save the file's name as a kill." (kill-save! s path #f #f))

(define-action buffer (show-buffer s d) "Show the buffer." (show-document! s d))
(define-action buffer (show-buffer-other-pane s d) "Show the buffer in a new pane below." (show-in-other-pane! s d))
(define-action buffer (save-buffer-document s d) "Write the buffer to its file." (document-save! d))
(define-action buffer (kill-buffer-target s d) "Take the buffer off the buffer list." (drop-buffer! s d))

;; The focused pane shows the location's document with the caret there.
(define (goto-location! s l show)
  (let* ((p (location-position l)) (v (show s (location-document l))))
    (view-set-ranges! v (list (list p p)) 0)))

(define-action location (goto-location s l) "Go to the location." (goto-location! s l show-document!))
(define-action location (goto-location-other-pane s l) "Go to the location in a new pane below." (goto-location! s l show-in-other-pane!))
(define-action location (copy-location-line s l)
  "Save the location's line as a kill."
  (kill-save! s (line-candidate-text (location-document l) (location-position l)) #f #f))
