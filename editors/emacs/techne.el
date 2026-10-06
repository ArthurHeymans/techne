;;; techne.el --- Interaction with techne Lisp over nREPL  -*- lexical-binding: t; -*-

;; Version: 0.1
;; Package-Requires: ((emacs "29.1"))
;; Keywords: languages, lisp

;;; Commentary:

;; A small client for the nREPL server of techne Lisp (`techne-node
;; --nrepl PORT').  It needs nothing beyond Emacs.
;;
;; M-x techne-jack-in starts a server in the current project and connects;
;; M-x techne-connect connects to a running one.  `techne-mode', a minor
;; mode for Scheme buffers, provides:
;;
;;   C-x C-e  evaluate the expression before point   C-M-x  the top-level form
;;   C-c C-r  evaluate the region      C-c C-k  load the buffer
;;   C-c C-z  the REPL                 C-c C-c  interrupt
;;   C-c C-i  inspect a value          C-c C-d  describe a name
;;
;; plus completion at point, eldoc and xref (M-.) from the server's
;; docstrings, parameter lists and definition sites.
;;
;; Errors that have restarts open a debugger buffer: the evaluation is
;; paused where the error was raised, and you pick a restart (with
;; arguments if it takes any) or abort.  Meanwhile you can still evaluate
;; and inspect.  The inspector shows a value's labelled parts; RET on a
;; part descends, `l' goes back.

;;; Code:

(require 'cl-lib)
(require 'subr-x)
(require 'xref)
(require 'eldoc)

(defgroup techne nil
  "Interaction with techne Lisp."
  :group 'languages)

(defcustom techne-node-program "techne-node"
  "The techne-node executable, for `techne-jack-in'."
  :type 'string)

(defcustom techne-request-timeout 10
  "Seconds to wait for synchronous requests (completion, lookup)."
  :type 'number)

;;;; Bencode

(defun techne--bencode (value)
  "Encode VALUE: an integer, a string, or an alist (a dictionary)."
  (cond
   ((integerp value) (format "i%de" value))
   ((stringp value)
    (let ((bytes (encode-coding-string value 'utf-8 t)))
      (concat (number-to-string (length bytes)) ":" bytes)))
   ((listp value)
    (concat "d"
            (mapconcat (lambda (entry)
                         (concat (techne--bencode (car entry)) (techne--bencode (cdr entry))))
                       (sort (copy-sequence value) (lambda (a b) (string< (car a) (car b))))
                       "")
            "e"))
   (t (error "Cannot bencode %S" value))))

(defun techne--bdecode (bytes pos)
  "Decode the value at POS in unibyte BYTES.
Return (VALUE . NEXT-POS), or nil if BYTES ends before the value does.
Dictionaries become alists with string keys; strings are decoded as UTF-8."
  (when (< pos (length bytes))
    (let ((c (aref bytes pos)))
      (cond
       ((eq c ?i)
        (let ((end (cl-position ?e bytes :start pos)))
          (when end
            (cons (string-to-number (substring bytes (1+ pos) end)) (1+ end)))))
       ((and (>= c ?0) (<= c ?9))
        (let ((colon (cl-position ?: bytes :start pos)))
          (when colon
            (let* ((len (string-to-number (substring bytes pos colon)))
                   (start (1+ colon))
                   (end (+ start len)))
              (when (<= end (length bytes))
                (cons (decode-coding-string (substring bytes start end) 'utf-8) end))))))
       ((memq c '(?l ?d))
        (let ((p (1+ pos)) items done)
          (while (and (not done) p)
            (cond ((>= p (length bytes)) (setq p nil))
                  ((eq (aref bytes p) ?e) (setq done t))
                  (t (let ((r (techne--bdecode bytes p)))
                       (if r (progn (push (car r) items) (setq p (cdr r))) (setq p nil))))))
          (when done
            (setq items (nreverse items))
            (cons (if (eq c ?l)
                      items
                    (cl-loop for (k v) on items by #'cddr collect (cons k v)))
                  (1+ p)))))
       (t (error "Bad bencode at %d" pos))))))

;;;; Connection

(cl-defstruct (techne--conn (:constructor techne--make-conn))
  process (pending "") (callbacks (make-hash-table :test #'equal)) session (next 1) server)

(defvar techne--connection nil
  "The current connection.")

(defun techne--get (msg key)
  "The value of KEY in message MSG."
  (cdr (assoc key msg)))

(defun techne--status-p (msg status)
  "Whether MSG's status list contains STATUS."
  (member status (techne--get msg "status")))

(defun techne--filter (proc output)
  "Decode the messages in OUTPUT from PROC and dispatch them."
  (let ((conn (process-get proc 'techne-conn)))
    (setf (techne--conn-pending conn) (concat (techne--conn-pending conn) output))
    (let (r)
      (while (setq r (techne--bdecode (techne--conn-pending conn) 0))
        (setf (techne--conn-pending conn) (substring (techne--conn-pending conn) (cdr r)))
        (let* ((msg (car r))
               (id (techne--get msg "id"))
               (callback (gethash id (techne--conn-callbacks conn))))
          (when (techne--status-p msg "done")
            (remhash id (techne--conn-callbacks conn)))
          (when callback
            (funcall callback msg)))))))

(defun techne--connection ()
  "The live connection, or an error."
  (unless (and techne--connection (process-live-p (techne--conn-process techne--connection)))
    (user-error "Not connected to techne: M-x techne-jack-in or M-x techne-connect"))
  techne--connection)

(defun techne-send (request callback &optional no-session)
  "Send REQUEST (an alist) and call CALLBACK with each reply.
Uses the connection's session unless NO-SESSION.  Returns the request id."
  (let* ((conn (techne--connection))
         (id (number-to-string (techne--conn-next conn))))
    (cl-incf (techne--conn-next conn))
    (push (cons "id" id) request)
    (when (and (not no-session) (techne--conn-session conn) (not (assoc "session" request)))
      (push (cons "session" (techne--conn-session conn)) request))
    (puthash id (or callback #'ignore) (techne--conn-callbacks conn))
    (process-send-string (techne--conn-process conn) (techne--bencode request))
    id))

(defun techne-request-sync (request &optional no-session)
  "Send REQUEST and return all its replies once one says done."
  (let ((replies nil) (done nil)
        (deadline (+ (float-time) techne-request-timeout)))
    (techne-send request (lambda (msg)
                           (push msg replies)
                           (when (techne--status-p msg "done") (setq done t)))
                 no-session)
    (while (and (not done) (< (float-time) deadline))
      (accept-process-output (techne--conn-process (techne--connection)) 0.05))
    (unless done (error "techne: no reply to %s" (cdr (assoc "op" request))))
    (nreverse replies)))

(defun techne--field (replies key)
  "The first value of KEY in REPLIES."
  (cl-some (lambda (m) (techne--get m key)) replies))

;;;###autoload
(defun techne-connect (host port)
  "Connect to a techne nREPL server at HOST and PORT."
  (interactive (list (read-string "Host: " "127.0.0.1")
                     (read-number "Port: " (techne--port-file-port))))
  (when techne--connection (ignore-errors (techne-disconnect)))
  (let* ((proc (make-network-process :name "techne" :host host :service port
                                     :coding 'binary :filter #'techne--filter :noquery t))
         (conn (techne--make-conn :process proc)))
    (process-put proc 'techne-conn conn)
    (setq techne--connection conn)
    (let ((replies (techne-request-sync '(("op" . "clone")) t)))
      (setf (techne--conn-session conn) (techne--field replies "new-session")))
    (message "techne: connected to %s:%s" host port)
    conn))

(defun techne--port-file-port ()
  "The port in the project's .nrepl-port file, if any."
  (let ((dir (locate-dominating-file default-directory ".nrepl-port")))
    (when dir
      (with-temp-buffer
        (insert-file-contents (expand-file-name ".nrepl-port" dir))
        (string-to-number (buffer-string))))))

;;;###autoload
(defun techne-jack-in ()
  "Start `techne-node --nrepl' in the current directory and connect."
  (interactive)
  (let* ((buffer (generate-new-buffer "*techne-server*"))
         (proc (make-process :name "techne-server" :buffer buffer
                             :command (list techne-node-program "--nrepl" "0")
                             :noquery t))
         (deadline (+ (float-time) 20))
         port)
    (while (and (not port) (< (float-time) deadline) (process-live-p proc))
      (accept-process-output proc 0.1)
      (with-current-buffer buffer
        (goto-char (point-min))
        (when (re-search-forward "nREPL server started on port \\([0-9]+\\)" nil t)
          (setq port (string-to-number (match-string 1))))))
    (unless port (error "techne: the server did not start (see %s)" (buffer-name buffer)))
    (let ((conn (techne-connect "127.0.0.1" port)))
      (setf (techne--conn-server conn) proc)
      conn)))

(defun techne-disconnect ()
  "Close the connection (and stop a server started by `techne-jack-in')."
  (interactive)
  (when techne--connection
    (let ((server (techne--conn-server techne--connection)))
      (delete-process (techne--conn-process techne--connection))
      (when (process-live-p server) (delete-process server)))
    (setq techne--connection nil)))

;;;; Evaluation

(defvar techne-repl-buffer-name "*techne-repl*")

(defvar-local techne--repl-prompt-start nil
  "Where output goes: just before the prompt.")
(defvar-local techne--repl-input-start nil
  "Where the input starts: just after the prompt.")

(defun techne--repl-output (text &optional face)
  "Show TEXT in the REPL buffer, above the prompt."
  (with-current-buffer (techne--repl-buffer)
    (save-excursion
      (let ((inhibit-read-only t))
        (goto-char techne--repl-prompt-start)
        (insert (if face (propertize text 'face face) text))
        (set-marker techne--repl-prompt-start (point))))))

(defun techne--eval-handler (&optional on-value)
  "A callback for eval replies; ON-VALUE receives the value string."
  (lambda (msg)
    (when-let* ((out (techne--get msg "out"))) (techne--repl-output out))
    (when-let* ((err (techne--get msg "err")))
      (techne--repl-output err 'error)
      (message "%s" (string-trim err)))
    (when-let* ((value (techne--get msg "value")))
      (if on-value (funcall on-value value) (message "=> %s" value)))
    (when (techne--status-p msg "techne-debug")
      (techne--debugger msg))
    (when (techne--status-p msg "interrupted")
      (message "techne: interrupted"))))

(defun techne-eval-string (code &optional on-value start)
  "Evaluate CODE on the server; ON-VALUE receives the value.
START is the buffer position CODE starts at, for definition sites."
  (let ((request `(("op" . "eval") ("code" . ,code) ("techne-debug" . 1))))
    (when (and start buffer-file-name)
      (save-excursion
        (goto-char start)
        (setq request (append request `(("file" . ,buffer-file-name)
                                        ("line" . ,(line-number-at-pos))
                                        ("column" . ,(1+ (current-column))))))))
    (techne-send request (techne--eval-handler on-value))))

(defun techne-eval-last-sexp ()
  "Evaluate the expression before point."
  (interactive)
  (let ((start (save-excursion (backward-sexp) (point))))
    (techne-eval-string (buffer-substring-no-properties start (point)) nil start)))

(defun techne-eval-defun ()
  "Evaluate the top-level form around point."
  (interactive)
  (save-excursion
    (end-of-defun)
    (let ((end (point)))
      (beginning-of-defun)
      (techne-eval-string (buffer-substring-no-properties (point) end) nil (point)))))

(defun techne-eval-region (start end)
  "Evaluate the region between START and END."
  (interactive "r")
  (techne-eval-string (buffer-substring-no-properties start end) nil start))

(defun techne-load-buffer ()
  "Load the whole buffer."
  (interactive)
  (techne-send `(("op" . "load-file")
                 ("file" . ,(buffer-substring-no-properties (point-min) (point-max)))
                 ("file-path" . ,(or buffer-file-name (buffer-name))))
               (techne--eval-handler (lambda (v) (message "Loaded => %s" v)))))

(defun techne-interrupt ()
  "Interrupt the running evaluation (or abort a paused one)."
  (interactive)
  (techne-send '(("op" . "interrupt")) #'ignore))

;;;; REPL

(defvar techne-repl-mode-map
  (let ((map (make-sparse-keymap)))
    (define-key map (kbd "RET") #'techne-repl-return)
    (define-key map (kbd "C-c C-c") #'techne-interrupt)
    map))

(define-derived-mode techne-repl-mode fundamental-mode "Techne REPL"
  "The techne REPL."
  (setq-local completion-at-point-functions (list #'techne-completion-at-point))
  (setq-local eldoc-documentation-functions (list #'techne-eldoc-function))
  (eldoc-mode 1))

(defun techne--repl-buffer ()
  "The REPL buffer, created if needed."
  (or (get-buffer techne-repl-buffer-name)
      (with-current-buffer (get-buffer-create techne-repl-buffer-name)
        (techne-repl-mode)
        (setq techne--repl-prompt-start (point-min-marker))
        (techne--repl-prompt)
        (current-buffer))))

(defun techne--repl-prompt ()
  "Insert a fresh prompt at the end."
  (let ((inhibit-read-only t))
    (goto-char (point-max))
    (set-marker techne--repl-prompt-start (point))
    (insert (propertize "λ> " 'read-only t 'rear-nonsticky t 'face 'comint-highlight-prompt 'field 'output))
    (setq techne--repl-input-start (point-marker))))

(defun techne-repl-return ()
  "Send the input after the prompt."
  (interactive)
  (let ((input (string-trim (buffer-substring-no-properties techne--repl-input-start (point-max))))
        (buffer (current-buffer)))
    (if (string-empty-p input)
        (insert "\n")
      (let ((inhibit-read-only t))
        (goto-char (point-max))
        (insert "\n")
        (add-text-properties techne--repl-prompt-start (point) '(read-only t)))
      (set-marker techne--repl-prompt-start (point))
      (setq techne--repl-input-start (point-marker))
      (techne-send `(("op" . "eval") ("code" . ,input) ("techne-debug" . 1))
                   (let ((handler (techne--eval-handler
                                   (lambda (v) (techne--repl-output (concat v "\n") 'font-lock-constant-face)))))
                     (lambda (msg)
                       (funcall handler msg)
                       (when (techne--status-p msg "done")
                         (with-current-buffer buffer (techne--repl-prompt)))))))))

(defun techne-switch-to-repl ()
  "Show the REPL."
  (interactive)
  (pop-to-buffer (techne--repl-buffer))
  (goto-char (point-max)))

;;;; Completion, eldoc, xref, help

(defun techne-completion-at-point ()
  "Complete the global name at point."
  (when-let* ((bounds (bounds-of-thing-at-point 'symbol))
              (_ techne--connection))
    (let* ((prefix (buffer-substring-no-properties (car bounds) (cdr bounds)))
           (replies (techne-request-sync `(("op" . "completions") ("prefix" . ,prefix))))
           (candidates (mapcar (lambda (c)
                                 (propertize (techne--get c "candidate") 'techne-type (techne--get c "type")))
                               (techne--field replies "completions"))))
      (list (car bounds) (cdr bounds) candidates
            :annotation-function (lambda (c) (when-let* ((type (get-text-property 0 'techne-type c))) (concat " " type)))
            :exclusive 'no))))

(defun techne--function-at-point ()
  "The name in function position of the list around point."
  (save-excursion
    (ignore-errors
      (backward-up-list)
      (forward-char)
      (thing-at-point 'symbol t))))

(defun techne-eldoc-function (callback &rest _)
  "Show the signature of the function being called, via CALLBACK."
  (when-let* ((name (techne--function-at-point))
              (_ (and techne--connection (process-live-p (techne--conn-process techne--connection)))))
    (techne-send `(("op" . "eldoc") ("sym" . ,name))
                 (lambda (msg)
                   (when-let* ((params (techne--get msg "eldoc")))
                     (funcall callback
                              (format "(%s%s)" name (mapconcat (lambda (p) (concat " " p)) (car params) ""))
                              :thing name :face 'font-lock-function-name-face))))
    t))

(defun techne--info (name)
  "The server's info about NAME, or nil."
  (let ((replies (techne-request-sync `(("op" . "info") ("sym" . ,name)))))
    (unless (cl-some (lambda (m) (techne--status-p m "no-info")) replies)
      (car replies))))

(defun techne-xref-backend () "The techne xref backend." (and techne--connection 'techne))

(cl-defmethod xref-backend-identifier-at-point ((_ (eql techne)))
  (thing-at-point 'symbol t))

(cl-defmethod xref-backend-definitions ((_ (eql techne)) name)
  (let* ((info (techne--info name))
         (file (techne--get info "file")))
    (when (and file (file-exists-p file))
      (list (xref-make name (xref-make-file-location
                             file (techne--get info "line") (max 0 (1- (or (techne--get info "column") 1)))))))))

(defun techne-describe (name)
  "Describe NAME: signature, documentation and definition site."
  (interactive (list (read-string "Describe: " (thing-at-point 'symbol t))))
  (let ((info (or (techne--info name) (user-error "No information about %s" name))))
    (with-help-window "*techne-help*"
      (princ (format "%s%s\n\n%s\n"
                     name
                     (if-let* ((args (techne--get info "arglists-str"))) (concat " " args) "")
                     (or (techne--get info "doc") "")))
      (when-let* ((file (techne--get info "file")))
        (princ (format "\nDefined at %s:%s\n" file (techne--get info "line")))))))

;;;; Debugger

(defvar techne-debug-mode-map
  (let ((map (make-sparse-keymap)))
    (dotimes (i 10)
      (define-key map (number-to-string i)
                  (lambda () (interactive) (techne-debug-invoke-restart i))))
    (define-key map "a" #'techne-debug-abort)
    (define-key map "q" #'techne-debug-abort)
    (define-key map "e" #'techne-debug-eval)
    map))

(define-derived-mode techne-debug-mode special-mode "Techne Debug"
  "A paused evaluation: choose a restart (0-9) or abort (a).")

(defvar-local techne--debug nil
  "The paused evaluation's debug message.")

(defun techne--debugger (msg)
  "Show the debugger for MSG (status techne-debug)."
  (let ((buffer (get-buffer-create "*techne-debug*")))
    (with-current-buffer buffer
      (techne-debug-mode)
      (setq techne--debug msg)
      (let ((inhibit-read-only t))
        (erase-buffer)
        (insert (propertize (techne--get msg "condition") 'face 'error) "\n\nRestarts:\n")
        (cl-loop for r in (techne--get msg "restarts") for i from 0 do
                 (insert-text-button (format "  [%d] %s" i (techne--get r "name"))
                                     'action (let ((i i)) (lambda (_) (techne-debug-invoke-restart i))))
                 (let ((params (techne--get r "params")))
                   (unless (string-empty-p params) (insert (format " (%s)" params))))
                 (insert "\n"))
        (insert-text-button "  [a] abort" 'action (lambda (_) (techne-debug-abort)))
        (insert "\n\nBacktrace:\n")
        (dolist (f (techne--get msg "frames")) (insert "  " f "\n"))
        (goto-char (point-min))))
    (pop-to-buffer buffer)))

(defun techne-debug-invoke-restart (index &optional args)
  "Continue the paused evaluation with restart INDEX.
ARGS is the source of the restart's arguments; asked for if it takes any."
  (interactive "nRestart: ")
  (let* ((msg (with-current-buffer "*techne-debug*" techne--debug))
         (restart (nth index (techne--get msg "restarts"))))
    (unless restart (user-error "No restart %d" index))
    (unless (or args (string-empty-p (techne--get restart "params")))
      (setq args (read-string (format "Arguments for %s (%s): " (techne--get restart "name") (techne--get restart "params")))))
    (techne-send `(("op" . "techne-debug-restart") ("debug-id" . ,(techne--get msg "debug-id"))
                   ("restart" . ,index) ("args" . ,(or args "")))
                 #'ignore)
    (techne--debug-close)))

(defun techne-debug-abort ()
  "Abort the paused evaluation."
  (interactive)
  (let ((msg (with-current-buffer "*techne-debug*" techne--debug)))
    (techne-send `(("op" . "techne-debug-abort") ("debug-id" . ,(techne--get msg "debug-id"))) #'ignore)
    (techne--debug-close)))

(defun techne-debug-eval (code)
  "Evaluate CODE while the evaluation is paused."
  (interactive "sEvaluate: ")
  (techne-eval-string code))

(defun techne--debug-close ()
  "Close the debugger buffer."
  (when-let* ((buffer (get-buffer "*techne-debug*")))
    (quit-windows-on buffer)
    (kill-buffer buffer)))

;;;; Inspector

(defvar techne-inspector-mode-map
  (let ((map (make-sparse-keymap)))
    (define-key map "l" #'techne-inspector-pop)
    (define-key map (kbd "TAB") #'forward-button)
    (define-key map (kbd "<backtab>") #'backward-button)
    map))

(define-derived-mode techne-inspector-mode special-mode "Techne Inspect"
  "A value and its parts: RET on a part inspects it, `l' goes back.")

(defun techne--inspector-show (replies)
  "Render the inspector view in REPLIES."
  (when-let* ((err (techne--field replies "err"))) (user-error "%s" (string-trim err)))
  (let ((buffer (get-buffer-create "*techne-inspect*")))
    (with-current-buffer buffer
      (techne-inspector-mode)
      (let ((inhibit-read-only t))
        (erase-buffer)
        (insert (propertize (techne--field replies "title") 'face 'bold)
                (format "  (depth %d)\n\n" (techne--field replies "depth"))
                (techne--field replies "value") "\n\n")
        (cl-loop for (label printed) in (techne--field replies "parts") for i from 0 do
                 (insert (propertize (format "%s: " label) 'face 'font-lock-variable-name-face))
                 (insert-text-button printed 'action (let ((i i)) (lambda (_) (techne-inspector-part i))))
                 (insert "\n"))
        (goto-char (point-min))))
    (pop-to-buffer buffer)))

(defun techne-inspect (code)
  "Inspect the value of CODE."
  (interactive (list (read-string "Inspect: " (thing-at-point 'sexp t))))
  (techne--inspector-show (techne-request-sync `(("op" . "techne-inspect") ("code" . ,code)))))

(defun techne-inspector-part (index)
  "Inspect part INDEX of the current value."
  (techne--inspector-show (techne-request-sync `(("op" . "techne-inspect-part") ("index" . ,index)))))

(defun techne-inspector-pop ()
  "Go back to the value containing this one."
  (interactive)
  (techne--inspector-show (techne-request-sync '(("op" . "techne-inspect-pop")))))

;;;; Minor mode

(defvar techne-mode-map
  (let ((map (make-sparse-keymap)))
    (define-key map (kbd "C-x C-e") #'techne-eval-last-sexp)
    (define-key map (kbd "C-M-x") #'techne-eval-defun)
    (define-key map (kbd "C-c C-r") #'techne-eval-region)
    (define-key map (kbd "C-c C-k") #'techne-load-buffer)
    (define-key map (kbd "C-c C-z") #'techne-switch-to-repl)
    (define-key map (kbd "C-c C-c") #'techne-interrupt)
    (define-key map (kbd "C-c C-i") #'techne-inspect)
    (define-key map (kbd "C-c C-d") #'techne-describe)
    map))

;;;###autoload
(define-minor-mode techne-mode
  "Evaluate, complete, document and navigate techne Lisp through the server."
  :lighter " Techne"
  (if techne-mode
      (progn
        (add-hook 'completion-at-point-functions #'techne-completion-at-point nil t)
        (add-hook 'eldoc-documentation-functions #'techne-eldoc-function nil t)
        (add-hook 'xref-backend-functions #'techne-xref-backend nil t))
    (remove-hook 'completion-at-point-functions #'techne-completion-at-point t)
    (remove-hook 'eldoc-documentation-functions #'techne-eldoc-function t)
    (remove-hook 'xref-backend-functions #'techne-xref-backend t)))

(provide 'techne)
;;; techne.el ends here
