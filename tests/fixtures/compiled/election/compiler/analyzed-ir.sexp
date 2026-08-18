(analyzed-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports (add_voter . %add_voter.3) (advance . %advance.4)
   (set_topic . %set_topic.1) (vote$commit . %vote$commit.2)
   (vote$reveal . %vote$reveal.0))
 (contract-types)
 (kernel-declaration (%kernel.77 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%authority.21
       (0)
       (exported #f)
       (__compact_Cell (tbytes 32)))
     (%state.22
       (1)
       (exported #f)
       (__compact_Cell
         (tenum PublicState setup commit reveal final)))
     (%topic.20
       (2)
       (exported #f)
       (__compact_Cell
         (tstruct
           Maybe
           (is_some (tboolean))
           (value (topaque "string")))))
     (%tally_yes.57 (3) (exported #f) (Counter))
     (%tally_no.59 (4) (exported #f) (Counter))
     (%committed_votes.41
       (5)
       (exported #f)
       (MerkleTree 10 (tbytes 32)))
     (%eligible_voters.26
       (6)
       (exported #f)
       (MerkleTree 10 (tbytes 32)))
     (%committed.42 (7) (exported #f) (Set (tbytes 32)))
     (%revealed.54 (8) (exported #f) (Set (tbytes 32))))
   (constructor () (tuple)))
 (circuit %merkleTreePathRoot.44 (exported #f) (pure #t) (proof #f)
   ((%path.71
      (tstruct
        MerkleTreePath
        (leaf (tbytes 32))
        (path
          (tvector
            10
            (tstruct
              MerkleTreePathEntry
              (sibling
                (tstruct MerkleTreeDigest (field (tfield (field-native)))))
              (goes_left (tboolean))))))))
   (tstruct MerkleTreeDigest (field (tfield (field-native))))
   (return
     (new (tstruct
            MerkleTreeDigest
            (field (tfield (field-native))))
          (fold
            10
            (fref %merkleTreePathEntryRoot.72)
            ((call
               %degradeToTransient.65
               (call
                 %persistentHash.69
                 (new (tstruct
                        LeafPreimage
                        (domain_sep (tbytes 6))
                        (data (tbytes 32)))
                      '#vu8(109 100 110 58 108 104)
                      (elt-ref (var-ref %path.71) leaf 0))))
              (tfield (field-native)))
            ((elt-ref (var-ref %path.71) path 1)
              (tvector
                10
                (tstruct
                  MerkleTreePathEntry
                  (sibling
                    (tstruct
                      MerkleTreeDigest
                      (field (tfield (field-native)))))
                  (goes_left (tboolean))))
              (tstruct
                MerkleTreePathEntry
                (sibling
                  (tstruct
                    MerkleTreeDigest
                    (field (tfield (field-native)))))
                (goes_left (tboolean))))))))
 (circuit %merkleTreePathEntryRoot.72 (exported #f) (pure #t)
   (proof #f)
   ((%recursiveDigest.73 (tfield (field-native)))
     (%entry.74
       (tstruct
         MerkleTreePathEntry
         (sibling
           (tstruct MerkleTreeDigest (field (tfield (field-native)))))
         (goes_left (tboolean)))))
   (tfield (field-native))
   (let* (((%left.75 (tfield (field-native))) (if (elt-ref
                                                    (var-ref %entry.74)
                                                    goes_left
                                                    1)
                                                  (var-ref
                                                    %recursiveDigest.73)
                                                  (elt-ref
                                                    (elt-ref
                                                      (var-ref %entry.74)
                                                      sibling
                                                      0)
                                                    field
                                                    0))))
     (let* (((%right.76 (tfield (field-native))) (if (elt-ref
                                                       (var-ref %entry.74)
                                                       goes_left
                                                       1)
                                                     (elt-ref
                                                       (elt-ref
                                                         (var-ref
                                                           %entry.74)
                                                         sibling
                                                         0)
                                                       field
                                                       0)
                                                     (var-ref
                                                       %recursiveDigest.73))))
       (return
         (call
           %transientHash.67
           (tuple
             (single (var-ref %left.75))
             (single (var-ref %right.76))))))))
 (native %transientHash.67
   (entry "__compactRuntime.transientHash" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.68 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native %persistentHash.69
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments
     (tstruct
       LeafPreimage
       (domain_sep (tbytes 6))
       (data (tbytes 32))))
   ((%value.70
      (tstruct
        LeafPreimage
        (domain_sep (tbytes 6))
        (data (tbytes 32)))))
   (tbytes 32))
 (native %persistentHash.7
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tvector 2 (tbytes 32)))
   ((%value.64 (tvector 2 (tbytes 32)))) (tbytes 32))
 (native %degradeToTransient.65
   (entry "__compactRuntime.degradeToTransient" circuit)
   (type-arguments) ((%x.66 (tbytes 32)))
   (tfield (field-native)))
 (witness %private$secret_key.17 () (tbytes 32))
 (witness
   %private$state.45
   ()
   (tenum PrivateState initial committed revealed))
 (witness %private$state$advance.40 () (ttuple))
 (witness
   %private$vote$record.46
   ((%ballot.63 (tenum PermissibleVotes yes no)))
   (ttuple))
 (witness
   %private$vote.50
   ()
   (tenum PermissibleVotes yes no))
 (witness
   %context$eligible_voters$path_of.27
   ((%pk.62 (tbytes 32)))
   (tstruct
     Maybe
     (is_some (tboolean))
     (value
       (tstruct
         MerkleTreePath
         (leaf (tbytes 32))
         (path
           (tvector
             10
             (tstruct
               MerkleTreePathEntry
               (sibling
                 (tstruct
                   MerkleTreeDigest
                   (field (tfield (field-native)))))
               (goes_left (tboolean)))))))))
 (witness
   %context$committed_votes$path_of.53
   ((%cm.60 (tbytes 32)))
   (tstruct
     Maybe
     (is_some (tboolean))
     (value
       (tstruct
         MerkleTreePath
         (leaf (tbytes 32))
         (path
           (tvector
             10
             (tstruct
               MerkleTreePathEntry
               (sibling
                 (tstruct
                   MerkleTreeDigest
                   (field (tfield (field-native)))))
               (goes_left (tboolean)))))))))
 (circuit %ballot_repr.39 (exported #f) (pure #t) (proof #f)
   ((%ballot.61 (tenum PermissibleVotes yes no))) (tbytes 32)
   (return
     (if (== (tenum PermissibleVotes yes no)
             (var-ref %ballot.61)
             (enum-ref (tenum PermissibleVotes yes no) yes))
         '#vu8(121 101 115 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
               0 0 0 0 0 0 0 0)
         '#vu8(110 111 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
               0 0 0 0 0 0 0))))
 (circuit %vote$commit.2 (exported #t) (pure #f) (proof #t)
   ((%ballot.33 (tenum PermissibleVotes yes no))) (ttuple)
   (seq (seq (assert
               (if (== (tenum PublicState setup commit reveal final)
                       (public-ledger %state.22 read (1) read
                         (tenum PublicState setup commit reveal final)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 1 1))))
                           (popeq (cached #f) (result (void)))))
                       (enum-ref
                         (tenum PublicState setup commit reveal final)
                         commit))
                   (== (tenum PrivateState initial committed revealed)
                       (call %private$state.45)
                       (enum-ref
                         (tenum PrivateState initial committed revealed)
                         initial))
                   '#f)
               "In illegal state for committing")
             (call %private$vote$record.46 (var-ref %ballot.33))
             (let* (((%sk.34 (tbytes 32)) (call %private$secret_key.17)))
               (let* (((%com_nul.35 (tbytes 32)) (call
                                                   %commitment_nullifier.11
                                                   (var-ref %sk.34))))
                 (seq (assert
                        (if (public-ledger %committed.42 read (7) member (tboolean)
                              (instructions (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 7 1))))
                                (push
                                  (storage #f)
                                  (value
                                    (state-value
                                      cell
                                      (var-ref %com_nul.35))))
                                (member)
                                (popeq (cached #t) (result (void))))
                              (var-ref %com_nul.35))
                            '#f
                            '#t)
                        "Unexpected attempt to double use of nullifier")
                      (let* (((%pk.36 (tbytes 32)) (call
                                                     %public_key.5
                                                     (var-ref %sk.34))))
                        (let* (((%path.37
                                  (tstruct
                                    Maybe
                                    (is_some (tboolean))
                                    (value
                                      (tstruct
                                        MerkleTreePath
                                        (leaf (tbytes 32))
                                        (path
                                          (tvector
                                            10
                                            (tstruct
                                              MerkleTreePathEntry
                                              (sibling
                                                (tstruct
                                                  MerkleTreeDigest
                                                  (field
                                                    (tfield
                                                      (field-native)))))
                                              (goes_left (tboolean))))))))) (call
                                                                              %context$eligible_voters$path_of.27
                                                                              (var-ref
                                                                                %pk.36))))
                          (seq (assert
                                 (if (if (elt-ref
                                           (var-ref %path.37)
                                           is_some
                                           0)
                                         (let* (((%tmp.43
                                                   (tstruct
                                                     MerkleTreeDigest
                                                     (field
                                                       (tfield
                                                         (field-native))))) (call
                                                                              %merkleTreePathRoot.44
                                                                              (elt-ref
                                                                                (var-ref
                                                                                  %path.37)
                                                                                value
                                                                                1))))
                                           (public-ledger %eligible_voters.26 read (6)
                                             checkRoot (tboolean)
                                             (instructions (dup (n 0))
                                               (idx (cached #f)
                                                    (pushPath #f)
                                                    (path ((align 6 1))))
                                               (idx (cached #f)
                                                    (pushPath #f)
                                                    (path ((align 0 1))))
                                               (root)
                                               (push
                                                 (storage #f)
                                                 (value
                                                   (state-value
                                                     cell
                                                     (var-ref %tmp.43))))
                                               (eq)
                                               (popeq
                                                 (cached #t)
                                                 (result (void))))
                                             (var-ref %tmp.43)))
                                         '#f)
                                     (== (tbytes 32)
                                         (var-ref %pk.36)
                                         (elt-ref
                                           (elt-ref
                                             (var-ref %path.37)
                                             value
                                             1)
                                           leaf
                                           0))
                                     '#f)
                                 "Attempted to vote without authorization - need to add-voter")
                               (let* (((%cm.38 (tbytes 32)) (call
                                                              %commit_with_sk.8
                                                              (call
                                                                %ballot_repr.39
                                                                (var-ref
                                                                  %ballot.33))
                                                              (var-ref
                                                                %sk.34))))
                                 (seq (public-ledger %committed_votes.41 update (5)
                                        insert (ttuple)
                                        (instructions
                                          (idx (cached #f)
                                               (pushPath #t)
                                               (path ((align 5 1))))
                                          (idx (cached #f)
                                               (pushPath #t)
                                               (path ((align 0 1))))
                                          (dup (n 2))
                                          (idx (cached #f)
                                               (pushPath #f)
                                               (path ((align 1 1))))
                                          (push
                                            (storage #t)
                                            (value
                                              (state-value
                                                cell
                                                (leaf-hash
                                                  (var-ref %cm.38)))))
                                          (ins (cached #f) (n 1))
                                          (ins (cached #t) (n 1))
                                          (idx (cached #f)
                                               (pushPath #t)
                                               (path ((align 1 1))))
                                          (addi (immediate 1))
                                          (ins (cached #t) (n 2)))
                                        (var-ref %cm.38))
                                      (public-ledger %committed.42 update (7) insert
                                        (ttuple)
                                        (instructions
                                          (idx (cached #f)
                                               (pushPath #t)
                                               (path ((align 7 1))))
                                          (push
                                            (storage #f)
                                            (value
                                              (state-value
                                                cell
                                                (var-ref %com_nul.35))))
                                          (push
                                            (storage #t)
                                            (value (state-value null)))
                                          (ins (cached #f) (n 1))
                                          (ins (cached #t) (n 1)))
                                        (var-ref %com_nul.35))
                                      (call
                                        %private$state$advance.40))))))))))
        (return (tuple))))
 (circuit %vote$reveal.0 (exported #t) (pure #f) (proof #t) ()
   (ttuple)
   (seq (seq (assert
               (if (== (tenum PublicState setup commit reveal final)
                       (public-ledger %state.22 read (1) read
                         (tenum PublicState setup commit reveal final)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 1 1))))
                           (popeq (cached #f) (result (void)))))
                       (enum-ref
                         (tenum PublicState setup commit reveal final)
                         reveal))
                   (== (tenum PrivateState initial committed revealed)
                       (call %private$state.45)
                       (enum-ref
                         (tenum PrivateState initial committed revealed)
                         committed))
                   '#f)
               "In illegal state for revealing")
             (let* (((%sk.47 (tbytes 32)) (call %private$secret_key.17)))
               (let* (((%rev_nul.48 (tbytes 32)) (call
                                                   %reveal_nullifier.13
                                                   (var-ref %sk.47))))
                 (seq (assert
                        (if (public-ledger %revealed.54 read (8) member (tboolean)
                              (instructions (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 8 1))))
                                (push
                                  (storage #f)
                                  (value
                                    (state-value
                                      cell
                                      (var-ref %rev_nul.48))))
                                (member)
                                (popeq (cached #t) (result (void))))
                              (var-ref %rev_nul.48))
                            '#f
                            '#t)
                        "Attempted to double vote")
                      (let* (((%vote.49 (tenum PermissibleVotes yes no)) (call
                                                                           %private$vote.50)))
                        (let* (((%cm.51 (tbytes 32)) (call
                                                       %commit_with_sk.8
                                                       (call
                                                         %ballot_repr.39
                                                         (var-ref
                                                           %vote.49))
                                                       (var-ref %sk.47))))
                          (let* (((%path.52
                                    (tstruct
                                      Maybe
                                      (is_some (tboolean))
                                      (value
                                        (tstruct
                                          MerkleTreePath
                                          (leaf (tbytes 32))
                                          (path
                                            (tvector
                                              10
                                              (tstruct
                                                MerkleTreePathEntry
                                                (sibling
                                                  (tstruct
                                                    MerkleTreeDigest
                                                    (field
                                                      (tfield
                                                        (field-native)))))
                                                (goes_left
                                                  (tboolean))))))))) (call
                                                                       %context$committed_votes$path_of.53
                                                                       (var-ref
                                                                         %cm.51))))
                            (seq (assert
                                   (if (if (elt-ref
                                             (var-ref %path.52)
                                             is_some
                                             0)
                                           (let* (((%tmp.55
                                                     (tstruct
                                                       MerkleTreeDigest
                                                       (field
                                                         (tfield
                                                           (field-native))))) (call
                                                                                %merkleTreePathRoot.44
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %path.52)
                                                                                  value
                                                                                  1))))
                                             (public-ledger %committed_votes.41 read (5)
                                               checkRoot (tboolean)
                                               (instructions (dup (n 0))
                                                 (idx (cached #f)
                                                      (pushPath #f)
                                                      (path ((align 5 1))))
                                                 (idx (cached #f)
                                                      (pushPath #f)
                                                      (path ((align 0 1))))
                                                 (root)
                                                 (push
                                                   (storage #f)
                                                   (value
                                                     (state-value
                                                       cell
                                                       (var-ref %tmp.55))))
                                                 (eq)
                                                 (popeq
                                                   (cached #t)
                                                   (result (void))))
                                               (var-ref %tmp.55)))
                                           '#f)
                                       (== (tbytes 32)
                                           (var-ref %cm.51)
                                           (elt-ref
                                             (elt-ref
                                               (var-ref %path.52)
                                               value
                                               1)
                                             leaf
                                             0))
                                       '#f)
                                   "Attempted to reveal incorrectly")
                                 (if (== (tenum PermissibleVotes yes no)
                                         (var-ref %vote.49)
                                         (enum-ref
                                           (tenum PermissibleVotes yes no)
                                           yes))
                                     (let* (((%tmp.56 (tunsigned 65535)) (safe-cast
                                                                           (tunsigned
                                                                             65535)
                                                                           (tunsigned
                                                                             1)
                                                                           '1)))
                                       (public-ledger %tally_yes.57 update (3) increment
                                         (ttuple)
                                         (instructions
                                           (idx (cached #f)
                                                (pushPath #t)
                                                (path ((align 3 1))))
                                           (addi
                                             (immediate
                                               (value->int
                                                 (var-ref %tmp.56))))
                                           (ins (cached #t) (n 1)))
                                         (var-ref %tmp.56)))
                                     (let* (((%tmp.58 (tunsigned 65535)) (safe-cast
                                                                           (tunsigned
                                                                             65535)
                                                                           (tunsigned
                                                                             1)
                                                                           '1)))
                                       (public-ledger %tally_no.59 update (4) increment
                                         (ttuple)
                                         (instructions
                                           (idx (cached #f)
                                                (pushPath #t)
                                                (path ((align 4 1))))
                                           (addi
                                             (immediate
                                               (value->int
                                                 (var-ref %tmp.58))))
                                           (ins (cached #t) (n 1)))
                                         (var-ref %tmp.58))))
                                 (public-ledger %revealed.54 update (8) insert (ttuple)
                                   (instructions
                                     (idx (cached #f)
                                          (pushPath #t)
                                          (path ((align 8 1))))
                                     (push
                                       (storage #f)
                                       (value
                                         (state-value
                                           cell
                                           (var-ref %rev_nul.48))))
                                     (push
                                       (storage #t)
                                       (value (state-value null)))
                                     (ins (cached #f) (n 1))
                                     (ins (cached #t) (n 1)))
                                   (var-ref %rev_nul.48))
                                 (call %private$state$advance.40)))))))))
        (return (tuple))))
 (circuit %advance.4 (exported #t) (pure #f) (proof #t) () (ttuple)
   (seq (let* (((%sk.28 (tbytes 32)) (call
                                       %private$secret_key.17)))
          (let* (((%apk.29 (tbytes 32)) (call
                                          %public_key.5
                                          (var-ref %sk.28))))
            (seq (assert
                   (== (tbytes 32)
                       (var-ref %apk.29)
                       (public-ledger %authority.21 read (0) read (tbytes 32)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 0 1))))
                           (popeq (cached #f) (result (void))))))
                   "Attempted to advance state without authorization")
                 (assert
                   (elt-ref
                     (public-ledger %topic.20 read (2) read
                       (tstruct
                         Maybe
                         (is_some (tboolean))
                         (value (topaque "string")))
                       (instructions
                         (dup (n 0))
                         (idx (cached #f)
                              (pushPath #f)
                              (path ((align 2 1))))
                         (popeq (cached #f) (result (void)))))
                     is_some
                     0)
                   "Attempted to start election without a topic")
                 (let* (((%tmp.30
                           (tenum PublicState setup commit reveal final)) (call
                                                                            %successor.31
                                                                            (public-ledger
                                                                              %state.22
                                                                              read
                                                                              (1)
                                                                              read
                                                                              (tenum
                                                                                PublicState
                                                                                setup
                                                                                commit
                                                                                reveal
                                                                                final)
                                                                              (instructions
                                                                                (dup (n 0))
                                                                                (idx (cached
                                                                                       #f)
                                                                                     (pushPath
                                                                                       #f)
                                                                                     (path
                                                                                       ((align
                                                                                          1
                                                                                          1))))
                                                                                (popeq
                                                                                  (cached
                                                                                    #f)
                                                                                  (result
                                                                                    (void))))))))
                   (public-ledger %state.22 write (1) write (ttuple)
                     (instructions
                       (push
                         (storage #f)
                         (value (state-value cell (align 1 1))))
                       (push
                         (storage #t)
                         (value (state-value cell (var-ref %tmp.30))))
                       (ins (cached #f) (n 1)))
                     (var-ref %tmp.30))))))
        (return (tuple))))
 (circuit %successor.31 (exported #f) (pure #t) (proof #f)
   ((%state.32 (tenum PublicState setup commit reveal final)))
   (tenum PublicState setup commit reveal final)
   (if (== (tenum PublicState setup commit reveal final)
           (var-ref %state.32)
           (enum-ref
             (tenum PublicState setup commit reveal final)
             setup))
       (return
         (enum-ref
           (tenum PublicState setup commit reveal final)
           commit))
       (if (== (tenum PublicState setup commit reveal final)
               (var-ref %state.32)
               (enum-ref
                 (tenum PublicState setup commit reveal final)
                 commit))
           (return
             (enum-ref
               (tenum PublicState setup commit reveal final)
               reveal))
           (return
             (enum-ref
               (tenum PublicState setup commit reveal final)
               final)))))
 (circuit %set_topic.1 (exported #t) (pure #f) (proof #t)
   ((%t.15 (topaque "string"))) (ttuple)
   (seq (let* (((%sk.16 (tbytes 32)) (call
                                       %private$secret_key.17)))
          (let* (((%apk.18 (tbytes 32)) (call
                                          %public_key.5
                                          (var-ref %sk.16))))
            (seq (assert
                   (== (tbytes 32)
                       (var-ref %apk.18)
                       (public-ledger %authority.21 read (0) read (tbytes 32)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 0 1))))
                           (popeq (cached #f) (result (void))))))
                   "Attempted to set topic without authorization")
                 (assert
                   (== (tenum PublicState setup commit reveal final)
                       (public-ledger %state.22 read (1) read
                         (tenum PublicState setup commit reveal final)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 1 1))))
                           (popeq (cached #f) (result (void)))))
                       (enum-ref
                         (tenum PublicState setup commit reveal final)
                         setup))
                   "Attempted to set topic after setup phase")
                 (let* (((%tmp.19
                           (tstruct
                             Maybe
                             (is_some (tboolean))
                             (value (topaque "string")))) (new (tstruct
                                                                 Maybe
                                                                 (is_some
                                                                   (tboolean))
                                                                 (value
                                                                   (topaque
                                                                     "string")))
                                                               '#t
                                                               (var-ref
                                                                 %t.15))))
                   (public-ledger %topic.20 write (2) write (ttuple)
                     (instructions
                       (push
                         (storage #f)
                         (value (state-value cell (align 2 1))))
                       (push
                         (storage #t)
                         (value (state-value cell (var-ref %tmp.19))))
                       (ins (cached #f) (n 1)))
                     (var-ref %tmp.19))))))
        (return (tuple))))
 (circuit %add_voter.3 (exported #t) (pure #f) (proof #t)
   ((%pk.23 (tbytes 32))) (ttuple)
   (seq (seq (assert
               (if (elt-ref
                     (call
                       %context$eligible_voters$path_of.27
                       (var-ref %pk.23))
                     is_some
                     0)
                   '#f
                   '#t)
               "Attempted to add a voter twice")
             (let* (((%sk.24 (tbytes 32)) (call %private$secret_key.17)))
               (let* (((%apk.25 (tbytes 32)) (call
                                               %public_key.5
                                               (var-ref %sk.24))))
                 (seq (assert
                        (== (tbytes 32)
                            (var-ref %apk.25)
                            (public-ledger %authority.21 read (0) read (tbytes 32)
                              (instructions
                                (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 0 1))))
                                (popeq (cached #f) (result (void))))))
                        "Attempted to add a voter without authorization")
                      (assert
                        (== (tenum PublicState setup commit reveal final)
                            (public-ledger %state.22 read (1) read
                              (tenum PublicState setup commit reveal final)
                              (instructions
                                (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 1 1))))
                                (popeq (cached #f) (result (void)))))
                            (enum-ref
                              (tenum PublicState setup commit reveal final)
                              setup))
                        "Attempted to add a voter after setup phase")
                      (public-ledger %eligible_voters.26 update (6) insert (ttuple)
                        (instructions
                          (idx (cached #f)
                               (pushPath #t)
                               (path ((align 6 1))))
                          (idx (cached #f)
                               (pushPath #t)
                               (path ((align 0 1))))
                          (dup (n 2))
                          (idx (cached #f)
                               (pushPath #f)
                               (path ((align 1 1))))
                          (push
                            (storage #t)
                            (value
                              (state-value
                                cell
                                (leaf-hash (var-ref %pk.23)))))
                          (ins (cached #f) (n 1)) (ins (cached #t) (n 1))
                          (idx (cached #f)
                               (pushPath #t)
                               (path ((align 1 1))))
                          (addi (immediate 1)) (ins (cached #t) (n 2)))
                        (var-ref %pk.23))))))
        (return (tuple))))
 (circuit %commitment_nullifier.11 (exported #f) (pure #t) (proof #f)
   ((%sk.12 (tbytes 32))) (tbytes 32)
   (return
     (call
       %persistentHash.7
       (tuple
         (single
           '#vu8(108 97 114 101 115 58 101 108 101 99 116 105 111 110
                 58 99 109 45 110 117 108 58 0 0 0 0 0 0 0 0 0 0))
         (single (var-ref %sk.12))))))
 (circuit %reveal_nullifier.13 (exported #f) (pure #t) (proof #f)
   ((%sk.14 (tbytes 32))) (tbytes 32)
   (return
     (call
       %persistentHash.7
       (tuple
         (single
           '#vu8(108 97 114 101 115 58 101 108 101 99 116 105 111 110
                 58 114 118 45 110 117 108 58 0 0 0 0 0 0 0 0 0 0))
         (single (var-ref %sk.14))))))
 (circuit %public_key.5 (exported #f) (pure #t) (proof #f)
   ((%sk.6 (tbytes 32))) (tbytes 32)
   (return
     (call
       %persistentHash.7
       (tuple
         (single
           '#vu8(108 97 114 101 115 58 101 108 101 99 116 105 111 110
                 58 112 107 58 0 0 0 0 0 0 0 0 0 0 0 0 0 0))
         (single (var-ref %sk.6))))))
 (circuit %commit_with_sk.8 (exported #f) (pure #t) (proof #f)
   ((%ballot.9 (tbytes 32)) (%sk.10 (tbytes 32))) (tbytes 32)
   (return
     (call
       %persistentHash.7
       (tuple
         (single (var-ref %ballot.9))
         (single (var-ref %sk.10)))))))
