;;; M-x markdown-preview: the focused buffer's markdown, rendered beside
;;; it as rich text (rich.scm, markdown.scm). It follows the markdown as
;;; it changes, from wherever it is edited; editing the rendering edits
;;; the markdown, the markup it hides staying as it was.

(import (techne editor))

(define (previews) (filter (lambda (b) (and (eq? (buffer-mode b) 'rich-mode) (hash-table? (buffer-state b))
                                            (hash-table-ref/default (buffer-state b) 'markdown #f)))
                           (buffer-list)))

(define (preview-of d) (find (lambda (b) (document=? (hash-table-ref (buffer-state b) 'markdown) d)) (previews)))

(define (directory-of-path p)
  (let loop ((i (- (string-length p) 1)))
    (cond ((< i 0) #f) ((char=? (string-ref p i) #\/) (substring p 0 (+ i 1))) (else (loop (- i 1))))))

(define (render! b)
  (let ((d (hash-table-ref (buffer-state b) 'markdown)))
    ;; What it follows from: an edit after this is rendered.
    (hash-table-set! (buffer-state b) 'rendered (document-revision d))
    (markdown-render! b d #:directory (and (document-path d) (directory-of-path (document-path d))))))

;; Render B again whenever its markdown changes, a tenth of a second after
;; the change, so a burst of typing is rendered once; until B is killed,
;; which cancels the task (it would else wait for an edit that may never
;; come, keeping both documents).
(define (follow! b)
  (hash-table-set!
   (buffer-state b) 'follower
   (spawn (lambda ()
           (let ((d (hash-table-ref (buffer-state b) 'markdown)))
             (let loop ()
               (let ((now (document-await-change d (hash-table-ref (buffer-state b) 'rendered))))
                 (when (and now (memq b (buffer-list)))
                   (sleep 100)
                   (when (memq b (buffer-list))
                     (render! b)
                     (loop))))))))))

(add-hook! 'buffer-killed 'markdown-preview
           (lambda (s b)
             (let ((st (buffer-state b)))
               (when (and (hash-table? st) (hash-table-ref/default st 'follower #f))
                 (task-cancel (hash-table-ref st 'follower))
                 (hash-table-delete! st 'follower)))))

(define-command (markdown-preview s n)
  "Show this markdown buffer rendered, beside it.
The rendering follows the markdown as it changes; editing its text
edits the markdown."
  (let* ((src (or (session-buffer s) (error "No buffer here")))
         (d (buffer-document src))
         (b (or (preview-of d)
                (let ((st (make-hash-table)))
                  (hash-table-set! st 'markdown d)
                  (let ((b (make-rich-buffer! (string-append "*preview " (buffer-name src) "*") #:state st)))
                    (follow! b)
                    b)))))
    (render! b)
    (if (and (= (length (session-panes s)) 1) (not (eq? src b)))
        (let ((focus (session-focus s)))
          (run-command s 'split-window-right 1)
          (sset! s 'focus (+ focus 1))
          (show-buffer! s b)
          (sset! s 'focus focus))
        (display-buffer! s (buffer-document b)))))

;; A new generation follows the previews shown: the previous one's tasks
;; went with it.
(for-each (lambda (b) (follow! b) (render! b)) (previews))
