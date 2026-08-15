(normalized-ir (compiler-version "0.33.122") (language-version "0.25.107")
  (runtime-version "0.18.107")
  (exports (clear . %clear.17) (get . %get.18)
    (public_key . %public_key.15) (set . %set.16)
    (value . %value.14))
  (contract-types)
  (kernel-declaration (%kernel.36 () (exported #f) (Kernel)))
  (public-ledger-declaration
    (public-ledger-array
      (%authority.23
        (0)
        (exported #f)
        (__compact_Cell (tbytes 32)))
      (%value.14
        (1)
        (exported #t)
        (__compact_Cell (tfield (field-native))))
      (%state.22
        (2)
        (exported #f)
        (__compact_Cell (tenum STATE unset set))))
    (constructor
      ((%v.37 (tfield (field-native))))
      (seq (let* (((%sk.38 (tbytes 32)) (call
                                          %private$secret_key.20)))
             (seq (let* (((%tmp.39 (tbytes 32)) (call
                                                  %public_key.15
                                                  (var-ref %sk.38))))
                    (public-ledger %authority.23 (0) write (ttuple)
                      (instructions
                        (push
                          (storage #f)
                          (value (state-value cell (align 0 1))))
                        (push
                          (storage #t)
                          (value (state-value cell (var-ref %tmp.39))))
                        (ins (cached #f) (n 1)))
                      (var-ref %tmp.39)))
                  (public-ledger %value.14 (1) write (ttuple)
                    (instructions
                      (push
                        (storage #f)
                        (value (state-value cell (align 1 1))))
                      (push
                        (storage #t)
                        (value (state-value cell (var-ref %v.37))))
                      (ins (cached #f) (n 1)))
                    (var-ref %v.37))
                  (public-ledger %state.22 (2) write (ttuple)
                    (instructions
                      (push
                        (storage #f)
                        (value (state-value cell (align 2 1))))
                      (push
                        (storage #t)
                        (value
                          (state-value
                            cell
                            (enum-ref (tenum STATE unset set) set))))
                      (ins (cached #f) (n 1)))
                    (enum-ref (tenum STATE unset set) set))))
           (return (tuple)))))
  (export-typedef
    Maybe
    (T)
    (tstruct Maybe (is_some (tboolean)) (value T)))
  (circuit %some.31 (exported #f) (pure #t) (proof #f)
    ((%value.35 (tfield (field-native))))
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tfield (field-native))))
    (return
      (new (tstruct
             Maybe
             (is_some (tboolean))
             (value (tfield (field-native))))
           '#t
           (var-ref %value.35))))
  (circuit %none.32 (exported #f) (pure #t) (proof #f) ()
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tfield (field-native))))
    (return
      (new (tstruct
             Maybe
             (is_some (tboolean))
             (value (tfield (field-native))))
           '#f
           (default (tfield (field-native))))))
  (native
    %persistentHash.27
    (entry "__compactRuntime.persistentHash" circuit)
    ((%value.34 (tvector 2 (tbytes 32))))
    (tbytes 32))
  (witness %private$secret_key.20 () (tbytes 32))
  (circuit %in_state.25 (exported #f) (pure #f) (proof #f)
    ((%s.33 (tenum STATE unset set))) (tboolean)
    (return
      (== (tenum STATE unset set)
          (public-ledger %state.22 (2) read (tenum STATE unset set)
            (instructions
              (dup (n 0))
              (idx (cached #f) (pushPath #f) (path ((align 2 1))))
              (popeq (cached #f) (result (void)))))
          (var-ref %s.33))))
  (circuit %set.16 (exported #t) (pure #f) (proof #t)
    ((%v.28 (tfield (field-native)))) (ttuple)
    (seq (seq (assert
                (call
                  %in_state.25
                  (enum-ref (tenum STATE unset set) unset))
                "set: attempted to overwrite recorded value")
              (let* (((%sk.29 (tbytes 32)) (call %private$secret_key.20)))
                (let* (((%apk.30 (tbytes 32)) (call
                                                %public_key.15
                                                (var-ref %sk.29))))
                  (seq (public-ledger %authority.23 (0) write (ttuple)
                         (instructions
                           (push
                             (storage #f)
                             (value (state-value cell (align 0 1))))
                           (push
                             (storage #t)
                             (value (state-value cell (var-ref %apk.30))))
                           (ins (cached #f) (n 1)))
                         (var-ref %apk.30))
                       (public-ledger %value.14 (1) write (ttuple)
                         (instructions
                           (push
                             (storage #f)
                             (value (state-value cell (align 1 1))))
                           (push
                             (storage #t)
                             (value (state-value cell (var-ref %v.28))))
                           (ins (cached #f) (n 1)))
                         (var-ref %v.28))
                       (public-ledger %state.22 (2) write (ttuple)
                         (instructions
                           (push
                             (storage #f)
                             (value (state-value cell (align 2 1))))
                           (push
                             (storage #t)
                             (value
                               (state-value
                                 cell
                                 (enum-ref (tenum STATE unset set) set))))
                           (ins (cached #f) (n 1)))
                         (enum-ref (tenum STATE unset set) set))))))
         (return (tuple))))
  (circuit %get.18 (exported #t) (pure #f) (proof #t) ()
    (tstruct
      Maybe
      (is_some (tboolean))
      (value (tfield (field-native))))
    (return
      (if (call
            %in_state.25
            (enum-ref (tenum STATE unset set) set))
          (call
            %some.31
            (public-ledger %value.14 (1) read (tfield (field-native))
              (instructions
                (dup (n 0))
                (idx (cached #f) (pushPath #f) (path ((align 1 1))))
                (popeq (cached #f) (result (void))))))
          (call %none.32))))
  (circuit %clear.17 (exported #t) (pure #f) (proof #t) () (ttuple)
    (seq (seq (assert
                (call %in_state.25 (enum-ref (tenum STATE unset set) set))
                "clear: no value is currently recorded")
              (let* (((%sk.19 (tbytes 32)) (call %private$secret_key.20)))
                (let* (((%apk.21 (tbytes 32)) (call
                                                %public_key.15
                                                (var-ref %sk.19))))
                  (seq (assert
                         (== (tbytes 32)
                             (var-ref %apk.21)
                             (public-ledger %authority.23 (0) read (tbytes 32)
                               (instructions
                                 (dup (n 0))
                                 (idx (cached #f)
                                      (pushPath #f)
                                      (path ((align 0 1))))
                                 (popeq (cached #f) (result (void))))))
                         "clear: attempted clear without proper authorization")
                       (let* (((%tmp.24 (tbytes 32)) (default
                                                       (tbytes 32))))
                         (public-ledger %authority.23 (0) write (ttuple)
                           (instructions
                             (push
                               (storage #f)
                               (value (state-value cell (align 0 1))))
                             (push
                               (storage #t)
                               (value
                                 (state-value cell (var-ref %tmp.24))))
                             (ins (cached #f) (n 1)))
                           (var-ref %tmp.24)))
                       (public-ledger %value.14 (1) write (ttuple)
                         (instructions
                           (push
                             (storage #f)
                             (value (state-value cell (align 1 1))))
                           (push
                             (storage #t)
                             (value
                               (state-value
                                 cell
                                 (default (tfield (field-native))))))
                           (ins (cached #f) (n 1)))
                         (default (tfield (field-native))))
                       (public-ledger %state.22 (2) write (ttuple)
                         (instructions
                           (push
                             (storage #f)
                             (value (state-value cell (align 2 1))))
                           (push
                             (storage #t)
                             (value
                               (state-value
                                 cell
                                 (enum-ref
                                   (tenum STATE unset set)
                                   unset))))
                           (ins (cached #f) (n 1)))
                         (enum-ref (tenum STATE unset set) unset))))))
         (return (tuple))))
  (circuit %public_key.15 (exported #t) (pure #t) (proof #f)
    ((%sk.26 (tbytes 32))) (tbytes 32)
    (return
      (call
        %persistentHash.27
        (tuple
          (single
            '#vu8(108 97 114 101 115 58 116 105 110 121 58 112 107 58 0
                  0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0))
          (single (var-ref %sk.26)))))))
