;;; Headless editor tests for org-mode and org-indent-mode.
(import (techne test))
(test-file #:script #t #:environment 'editor)
(require "../main.scm")
(require "../session.scm")
(require "../dispatch.scm")
(require "../modes.scm")
(require "../api.scm")
(require "../builtins.scm")

(define (editing profile filename text)
  (let ((s (start-session (make-view (make-document text) "user") profile))
        (m (mode-for-file filename)))
    (when (and (eq? m 'org-mode) (not (find-package 'org-runtime)))
      (activate-command-package! 'org-runtime))
    (set-buffer-mode! (current-buffer s) m)
    s))

(define (text s) (document-string (session-document s)))

;; 1. Auto-mode detection for .org files
(check "mode-for-file .org" 'org-mode (mode-for-file "notes.org"))
(check "mode-for-file .md still text-mode" 'text-mode (mode-for-file "README.md"))

;; 1b. org-cycle (TAB) and org-shifttab (S-TAB) visibility cycling
(let* ((doc-str "#+TITLE: Cycle\n* Parent\nParent body\n** Child\nChild body\n")
       (s (editing "emacs" "cycle.org" doc-str))
       (v (session-view s)))
  ;; S-TAB cycles global visibility from anywhere: OVERVIEW -> CONTENTS -> SHOW ALL
  (press-keys s "S-TAB")
  (check-true "S-TAB folds to OVERVIEW" (pair? (buffer-folds v)))
  (press-keys s "S-TAB")
  (check-true "S-TAB advances to CONTENTS" (pair? (buffer-folds v)))
  (press-keys s "S-TAB")
  (check "S-TAB advances to SHOW ALL" '() (buffer-folds v))
  ;; TAB on a plain body line does not fold the enclosing heading (Emacs Org default)
  (let ((body-pos (string-contains doc-str "Parent body")))
    (view-set-ranges! v (list (list body-pos body-pos)) 0)
    (press-keys s "TAB")
    (check "TAB on body line does not fold heading" '() (buffer-folds v)))
  ;; Move to Parent: TAB -> FOLDED -> CHILDREN, then move to Child and TAB unfolds Child
  (let ((p-pos (string-contains doc-str "* Parent"))
        (c-pos (string-contains doc-str "** Child")))
    (view-set-ranges! v (list (list p-pos p-pos)) 0)
    (press-keys s "TAB")
    (check-true "Parent FOLDED hides subtree" (pair? (buffer-folds v)))
    (press-keys s "TAB")
    (check-true "Parent CHILDREN exposes Child headline" (pair? (buffer-folds v)))
    (view-set-ranges! v (list (list c-pos c-pos)) 0)
    (press-keys s "TAB")
    ;; Child body is now unfolded; only Parent body remains folded (1 range ending at c-pos).
    (check "Child TAB unfolds Child body independently" 1 (length (buffer-folds v)))))

;; 2. Highlighting layer & org-indent-mode inlays/projections
(let* ((sample (string-append
                "#+title: Demo\n"
                "* TODO [#A] Top task [1/2] :work:\n"
                "  SCHEDULED: <2026-11-26 Thu 10:00>\n"
                "** REVIEW Sub heading\n"
                "- [ ] item one with *bold* and [[https://example.com][link]]\n"
                "#+begin_src scheme\n"
                "(+ 1 2)\n"
                "#+end_src\n"))
       (s (editing "emacs" "demo.org" sample))
       (d (session-document s))
       (layers (buffer-layers d 0 (document-length d)))
       (faces (map caddr layers)))
  (check-true "has org.keyword face" (and (member 'org.keyword faces) #t))
  (check-true "has org.heading.1 face" (and (member 'org.heading.1 faces) #t))
  (check-true "has org.heading.2 face" (and (member 'org.heading.2 faces) #t))
  (check-true "has org.stars.leading face" (and (member 'org.stars.leading faces) #t))
  (check-true "has org.todo face" (and (member 'org.todo faces) #t))
  (check-true "has org.todo.review face" (and (member 'org.todo.review faces) #t))
  (check-true "has org.priority face" (and (member 'org.priority faces) #t))
  (check-true "has org.tag face" (and (member 'org.tag faces) #t))
  (check-true "has org.planning face" (and (member 'org.planning faces) #t))
  (check-true "has org.timestamp.active face" (and (member 'org.timestamp.active faces) #t))
  (check-true "has org.cookie face" (and (member 'org.cookie faces) #t))
  (check-true "has org.checkbox face" (and (member 'org.checkbox faces) #t))
  (check-true "has org.bold face" (and (member 'org.bold faces) #t))
  (check-true "has org.link face" (and (member 'org.link faces) #t))
  (check-true "has org.block.delimiter face" (and (member 'org.block.delimiter faces) #t))
  (check-true "has org.block.body face" (and (member 'org.block.body faces) #t))
  ;; Turn on org-indent-mode and verify inlays + hang projections appear
  (toggle-mode! s 'org-indent-mode)
  (check-true "org-indent-mode produces inlays" (pair? (buffer-inlays d 0 (document-length d))))
  (check-true "org-indent-mode produces hang projections" (pair? (buffer-projections d 0 (document-length d)))))

;; 3. Navigation: next/prev/forward-same-level/backward-same-level/up
(let ((s (editing "emacs" "nav.org" "* One\n** Child\n* Two\n")))
  (press-keys s "C-c C-n")
  (check "C-c C-n moves to Child" (string-contains (text s) "** Child") (view-point (session-view s)))
  (press-keys s "C-c C-u")
  (check "C-c C-u moves to parent One" 0 (view-point (session-view s)))
  (press-keys s "C-c C-f")
  (check "C-c C-f moves to sibling Two" (string-contains (text s) "* Two") (view-point (session-view s)))
  (press-keys s "C-c C-b")
  (check "C-c C-b moves back to One" 0 (view-point (session-view s))))

;; 4. TODO cycling, priority, checkbox toggle, undo
(let ((s (editing "emacs" "todo.org" "* Alpha [0/2]\n- [ ] first\n- [ ] second\n")))
  (set-option! 'org-log-done #f #:buffer (current-buffer s))
  (press-keys s "C-c C-t")
  (check "C-c C-t cycles to TODO" "* TODO Alpha [0/2]\n- [ ] first\n- [ ] second\n" (text s))
  (press-keys s "C-c ,")
  (check "C-c , sets priority #A" "* TODO [#A] Alpha [0/2]\n- [ ] first\n- [ ] second\n" (text s))
  (press-keys s "C-n C-c C-c")
  (check "C-c C-c toggles checkbox and updates cookie"
         "* TODO [#A] Alpha [1/2]\n- [X] first\n- [ ] second\n"
         (text s))
  (press-keys s "C-/")
  (check "undo restores checkbox and cookie in one transaction"
         "* TODO [#A] Alpha [0/2]\n- [ ] first\n- [ ] second\n"
         (text s)))

;; 5. Promote/demote & move subtree (and modal profile keybindings)
(let ((s (editing "modal" "struct.org" "* First\n** Sub\n* Second\n")))
  (press-keys s "M-<down>")
  (check "M-<down> swaps First and Second" "* Second\n* First\n** Sub\n" (text s))
  (press-keys s "M-S-<right>")
  (check "M-S-<right> demotes First subtree" "* Second\n** First\n*** Sub\n" (text s))
  (press-keys s "M-S-<left>")
  (check "M-S-<left> promotes First subtree" "* Second\n* First\n** Sub\n" (text s))
  (press-keys s "SPC m p")
  (check "modal SPC m p sets priority" "* Second\n* [#A] First\n** Sub\n" (text s)))

;; 6. Heading link following
(let ((s (editing "emacs" "links.org" "* Intro\nSee [[*Target][jump]].\n* Target\nDone.\n")))
  (let ((pos (string-contains (text s) "[[*Target]")))
    (view-set-ranges! (session-view s) (list (list pos pos)) 0)
    (press-keys s "C-c C-o")
    (check "C-c C-o jumps to *Target heading" (string-contains (text s) "* Target") (view-point (session-view s)))))

;; 7. Minibuffer-driven metadata commands (schedule, deadline, tags, property, effort)
(let ((s (editing "emacs" "meta.org" "* TODO Plan\n")))
  (press-keys s "C-c C-q")
  (type-text s "work:urgent")
  (press-keys s "RET")
  (check "C-c C-q sets tags via completing-read" "* TODO Plan :work:urgent:\n" (text s))
  (press-keys s "C-c C-x p")
  (type-text s "OWNER=arthur")
  (press-keys s "RET")
  (check "C-c C-x p sets property via completing-read"
         "* TODO Plan :work:urgent:\n:PROPERTIES:\n:OWNER: arthur\n:END:\n"
         (text s))
  (press-keys s "C-c C-x e")
  (type-text s "1:30")
  (press-keys s "RET")
  (check "C-c C-x e sets EFFORT property"
         "* TODO Plan :work:urgent:\n:PROPERTIES:\n:OWNER: arthur\n:EFFORT: 1:30\n:END:\n"
         (text s)))

;; 8. Per-view folding (TAB, S-TAB, drawer folding, caret-never-hidden, independent views)
(let* ((sample (string-append
                "* One\n"
                ":PROPERTIES:\n"
                ":ID: 1\n"
                ":END:\n"
                "Body one\n"
                "** Child\n"
                "Child body\n"
                "* Two\n"
                "Body two\n"))
       (s (editing "emacs" "fold.org" sample))
       (v1 (session-view s))
       (v2 (view-split v1)))
  ;; Fold * One in v1; v2 remains completely unfolded!
  (press-keys s "TAB")
  (check "TAB folds * One in v1"
         (list (list (string-contains sample ":PROPERTIES:") (string-contains sample "* Two")))
         (buffer-folds v1))
  (check "v2 has no folds (per-view isolation)" '() (buffer-folds v2))
  ;; C-n from * One skips the entire folded subtree and lands on * Two!
  (press-keys s "C-n")
  (check "C-n skips folded subtree to * Two" (string-contains sample "* Two") (view-point v1))
  ;; C-p moves back to * One!
  (press-keys s "C-p")
  (check "C-p skips folded subtree back to * One" 0 (view-point v1))
  ;; Second TAB on * One enters CHILDREN (body + child body folded, ** Child headline visible!)
  (press-keys s "TAB")
  (check "second TAB shows CHILDREN" 2 (length (buffer-folds v1)))
  (press-keys s "C-c C-n")
  (check "C-c C-n visits visible ** Child" (string-contains sample "** Child") (view-point v1))
  ;; Third TAB on * One unfolds subtree completely
  (view-set-ranges! v1 '((0 0)) 0)
  (press-keys s "TAB")
  (check "third TAB unfolds subtree" '() (buffer-folds v1))
  ;; TAB on :PROPERTIES: folds only the drawer
  (let ((prop-pos (string-contains sample ":PROPERTIES:")))
    (view-set-ranges! v1 (list (list prop-pos prop-pos)) 0)
    (press-keys s "TAB")
    (check "TAB folds :PROPERTIES: drawer"
           (list (list (string-contains sample ":ID: 1") (string-contains sample "Body one")))
           (buffer-folds v1))
    (press-keys s "C-c C-r")
    (check "C-c C-r reveals folded drawer" '() (buffer-folds v1)))
  ;; S-TAB cycles OVERVIEW -> CONTENTS -> SHOW ALL
  (press-keys s "S-TAB")
  (check "S-TAB OVERVIEW folds top-level headings" 2 (length (buffer-folds v1)))
  (press-keys s "S-TAB")
  (check "S-TAB CONTENTS shows all headlines" 3 (length (buffer-folds v1)))
  (press-keys s "S-TAB")
  (check "S-TAB SHOW ALL clears all folds" '() (buffer-folds v1)))

;; 7. S-TAB CONTENTS keeps grandchildren (*** C) visible while folding bodies
(let* ((sample "* A\nBody A\n** B\nBody B\n*** C\nBody C\n")
       (s (editing "emacs" "deep.org" sample))
       (v (session-view s))
       (c-pos (string-contains sample "*** C")))
  (press-keys s "S-TAB") ; OVERVIEW
  (press-keys s "S-TAB") ; CONTENTS
  (let ((folds (buffer-folds v)))
    (check "CONTENTS hides 3 bodies" 3 (length folds))
    (check "CONTENTS leaves *** C visible (not inside any fold)"
           #f
           (any (lambda (r) (and (> c-pos (car r)) (< c-pos (cadr r)))) folds))))


