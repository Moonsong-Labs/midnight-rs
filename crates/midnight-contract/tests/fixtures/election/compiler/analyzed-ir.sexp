(analyzed-ir (compiler-version "0.33.122")
 (language-version "0.25.107") (runtime-version "0.18.107")
 (exports (add_voter . %add_voter.136) (advance . %advance.137)
   (set_topic . %set_topic.134)
   (vote$commit . %vote$commit.135)
   (vote$reveal . %vote$reveal.133))
 (contract-types)
 (kernel-declaration (%kernel.203 () (exported #f) (Kernel)))
 (public-ledger-declaration
   (public-ledger-array
     (%authority.154
       (0)
       (exported #f)
       (__compact_Cell (tbytes 32)))
     (%state.155
       (1)
       (exported #f)
       (__compact_Cell
         (tenum PublicState setup commit reveal final)))
     (%topic.151
       (2)
       (exported #f)
       (__compact_Cell
         (tstruct
           Maybe
           (is_some (tboolean))
           (value (topaque "string")))))
     (%tally_yes.189 (3) (exported #f) (Counter))
     (%tally_no.191 (4) (exported #f) (Counter))
     (%committed_votes.171
       (5)
       (exported #f)
       (MerkleTree 10 (tbytes 32)))
     (%eligible_voters.157
       (6)
       (exported #f)
       (MerkleTree 10 (tbytes 32)))
     (%committed.173 (7) (exported #f) (Set (tbytes 32)))
     (%revealed.185 (8) (exported #f) (Set (tbytes 32))))
   (constructor () (tuple)))
 (circuit %merkleTreePathRoot.176 (exported #f) (pure #t) (proof #f)
   ((%path.198
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
            (fref %merkleTreePathEntryRoot.197)
            ((call
               %degradeToTransient.194
               (call
                 %persistentHash.196
                 (new (tstruct
                        LeafPreimage
                        (domain_sep (tbytes 6))
                        (data (tbytes 32)))
                      '#vu8(109 100 110 58 108 104)
                      (elt-ref (var-ref %path.198) leaf 0))))
              (tfield (field-native)))
            ((elt-ref (var-ref %path.198) path 1)
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
 (circuit %merkleTreePathEntryRoot.197 (exported #f) (pure #t)
   (proof #f)
   ((%recursiveDigest.200 (tfield (field-native)))
     (%entry.199
       (tstruct
         MerkleTreePathEntry
         (sibling
           (tstruct MerkleTreeDigest (field (tfield (field-native)))))
         (goes_left (tboolean)))))
   (tfield (field-native))
   (let* (((%left.201 (tfield (field-native))) (if (elt-ref
                                                     (var-ref %entry.199)
                                                     goes_left
                                                     1)
                                                   (var-ref
                                                     %recursiveDigest.200)
                                                   (elt-ref
                                                     (elt-ref
                                                       (var-ref %entry.199)
                                                       sibling
                                                       0)
                                                     field
                                                     0))))
     (let* (((%right.202 (tfield (field-native))) (if (elt-ref
                                                        (var-ref
                                                          %entry.199)
                                                        goes_left
                                                        1)
                                                      (elt-ref
                                                        (elt-ref
                                                          (var-ref
                                                            %entry.199)
                                                          sibling
                                                          0)
                                                        field
                                                        0)
                                                      (var-ref
                                                        %recursiveDigest.200))))
       (return
         (call
           %transientHash.195
           (tuple
             (single (var-ref %left.201))
             (single (var-ref %right.202))))))))
 (native %transientHash.195
   (entry "__compactRuntime.transientHash" circuit)
   (type-arguments (tvector 2 (tfield (field-native))))
   ((%value.204 (tvector 2 (tfield (field-native)))))
   (tfield (field-native)))
 (native %persistentHash.196
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments
     (tstruct
       LeafPreimage
       (domain_sep (tbytes 6))
       (data (tbytes 32))))
   ((%value.205
      (tstruct
        LeafPreimage
        (domain_sep (tbytes 6))
        (data (tbytes 32)))))
   (tbytes 32))
 (native %persistentHash.139
   (entry "__compactRuntime.persistentHash" circuit)
   (type-arguments (tvector 2 (tbytes 32)))
   ((%value.206 (tvector 2 (tbytes 32)))) (tbytes 32))
 (native %degradeToTransient.194
   (entry "__compactRuntime.degradeToTransient" circuit)
   (type-arguments) ((%x.207 (tbytes 32)))
   (tfield (field-native)))
 (witness %private$secret_key.148 () (tbytes 32))
 (witness
   %private$state.178
   ()
   (tenum PrivateState initial committed revealed))
 (witness %private$state$advance.170 () (ttuple))
 (witness
   %private$vote$record.179
   ((%ballot.208 (tenum PermissibleVotes yes no)))
   (ttuple))
 (witness
   %private$vote.181
   ()
   (tenum PermissibleVotes yes no))
 (witness
   %context$eligible_voters$path_of.160
   ((%pk.209 (tbytes 32)))
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
   %context$committed_votes$path_of.183
   ((%cm.210 (tbytes 32)))
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
 (circuit %ballot_repr.168 (exported #f) (pure #t) (proof #f)
   ((%ballot.193 (tenum PermissibleVotes yes no))) (tbytes 32)
   (return
     (if (== (tenum PermissibleVotes yes no)
             (var-ref %ballot.193)
             (enum-ref (tenum PermissibleVotes yes no) yes))
         '#vu8(121 101 115 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
               0 0 0 0 0 0 0 0)
         '#vu8(110 111 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
               0 0 0 0 0 0 0))))
 (circuit %vote$commit.135 (exported #t) (pure #f) (proof #t)
   ((%ballot.169 (tenum PermissibleVotes yes no))) (ttuple)
   (seq (seq (assert
               (if (== (tenum PublicState setup commit reveal final)
                       (public-ledger %state.155 read (1) read
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
                       (call %private$state.178)
                       (enum-ref
                         (tenum PrivateState initial committed revealed)
                         initial))
                   '#f)
               "In illegal state for committing")
             (call %private$vote$record.179 (var-ref %ballot.169))
             (let* (((%sk.166 (tbytes 32)) (call
                                             %private$secret_key.148)))
               (let* (((%com_nul.174 (tbytes 32)) (call
                                                    %commitment_nullifier.144
                                                    (var-ref %sk.166))))
                 (seq (assert
                        (if (public-ledger %committed.173 read (7) member (tboolean)
                              (instructions (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 7 1))))
                                (push
                                  (storage #f)
                                  (value
                                    (state-value
                                      cell
                                      (var-ref %com_nul.174))))
                                (member)
                                (popeq (cached #t) (result (void))))
                              (var-ref %com_nul.174))
                            '#f
                            '#t)
                        "Unexpected attempt to double use of nullifier")
                      (let* (((%pk.167 (tbytes 32)) (call
                                                      %public_key.138
                                                      (var-ref %sk.166))))
                        (let* (((%path.175
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
                                                                              %context$eligible_voters$path_of.160
                                                                              (var-ref
                                                                                %pk.167))))
                          (seq (assert
                                 (if (if (elt-ref
                                           (var-ref %path.175)
                                           is_some
                                           0)
                                         (let* (((%tmp.177
                                                   (tstruct
                                                     MerkleTreeDigest
                                                     (field
                                                       (tfield
                                                         (field-native))))) (call
                                                                              %merkleTreePathRoot.176
                                                                              (elt-ref
                                                                                (var-ref
                                                                                  %path.175)
                                                                                value
                                                                                1))))
                                           (public-ledger %eligible_voters.157 read (6)
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
                                                     (var-ref %tmp.177))))
                                               (eq)
                                               (popeq
                                                 (cached #t)
                                                 (result (void))))
                                             (var-ref %tmp.177)))
                                         '#f)
                                     (== (tbytes 32)
                                         (var-ref %pk.167)
                                         (elt-ref
                                           (elt-ref
                                             (var-ref %path.175)
                                             value
                                             1)
                                           leaf
                                           0))
                                     '#f)
                                 "Attempted to vote without authorization - need to add-voter")
                               (let* (((%cm.172 (tbytes 32)) (call
                                                               %commit_with_sk.141
                                                               (call
                                                                 %ballot_repr.168
                                                                 (var-ref
                                                                   %ballot.169))
                                                               (var-ref
                                                                 %sk.166))))
                                 (seq (public-ledger %committed_votes.171 update (5)
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
                                                  (var-ref %cm.172)))))
                                          (ins (cached #f) (n 1))
                                          (ins (cached #t) (n 1))
                                          (idx (cached #f)
                                               (pushPath #t)
                                               (path ((align 1 1))))
                                          (addi (immediate 1))
                                          (ins (cached #t) (n 2)))
                                        (var-ref %cm.172))
                                      (public-ledger %committed.173 update (7) insert
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
                                                (var-ref %com_nul.174))))
                                          (push
                                            (storage #t)
                                            (value (state-value null)))
                                          (ins (cached #f) (n 1))
                                          (ins (cached #t) (n 1)))
                                        (var-ref %com_nul.174))
                                      (call
                                        %private$state$advance.170))))))))))
        (return (tuple))))
 (circuit %vote$reveal.133 (exported #t) (pure #f) (proof #t) ()
   (ttuple)
   (seq (seq (assert
               (if (== (tenum PublicState setup commit reveal final)
                       (public-ledger %state.155 read (1) read
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
                       (call %private$state.178)
                       (enum-ref
                         (tenum PrivateState initial committed revealed)
                         committed))
                   '#f)
               "In illegal state for revealing")
             (let* (((%sk.180 (tbytes 32)) (call
                                             %private$secret_key.148)))
               (let* (((%rev_nul.186 (tbytes 32)) (call
                                                    %reveal_nullifier.146
                                                    (var-ref %sk.180))))
                 (seq (assert
                        (if (public-ledger %revealed.185 read (8) member (tboolean)
                              (instructions (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 8 1))))
                                (push
                                  (storage #f)
                                  (value
                                    (state-value
                                      cell
                                      (var-ref %rev_nul.186))))
                                (member)
                                (popeq (cached #t) (result (void))))
                              (var-ref %rev_nul.186))
                            '#f
                            '#t)
                        "Attempted to double vote")
                      (let* (((%vote.182 (tenum PermissibleVotes yes no)) (call
                                                                            %private$vote.181)))
                        (let* (((%cm.184 (tbytes 32)) (call
                                                        %commit_with_sk.141
                                                        (call
                                                          %ballot_repr.168
                                                          (var-ref
                                                            %vote.182))
                                                        (var-ref
                                                          %sk.180))))
                          (let* (((%path.187
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
                                                                       %context$committed_votes$path_of.183
                                                                       (var-ref
                                                                         %cm.184))))
                            (seq (assert
                                   (if (if (elt-ref
                                             (var-ref %path.187)
                                             is_some
                                             0)
                                           (let* (((%tmp.188
                                                     (tstruct
                                                       MerkleTreeDigest
                                                       (field
                                                         (tfield
                                                           (field-native))))) (call
                                                                                %merkleTreePathRoot.176
                                                                                (elt-ref
                                                                                  (var-ref
                                                                                    %path.187)
                                                                                  value
                                                                                  1))))
                                             (public-ledger %committed_votes.171 read
                                               (5) checkRoot (tboolean)
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
                                                       (var-ref
                                                         %tmp.188))))
                                                 (eq)
                                                 (popeq
                                                   (cached #t)
                                                   (result (void))))
                                               (var-ref %tmp.188)))
                                           '#f)
                                       (== (tbytes 32)
                                           (var-ref %cm.184)
                                           (elt-ref
                                             (elt-ref
                                               (var-ref %path.187)
                                               value
                                               1)
                                             leaf
                                             0))
                                       '#f)
                                   "Attempted to reveal incorrectly")
                                 (if (== (tenum PermissibleVotes yes no)
                                         (var-ref %vote.182)
                                         (enum-ref
                                           (tenum PermissibleVotes yes no)
                                           yes))
                                     (let* (((%tmp.190 (tunsigned 65535)) (safe-cast
                                                                            (tunsigned
                                                                              65535)
                                                                            (tunsigned
                                                                              1)
                                                                            '1)))
                                       (public-ledger %tally_yes.189 update (3)
                                         increment (ttuple)
                                         (instructions
                                           (idx (cached #f)
                                                (pushPath #t)
                                                (path ((align 3 1))))
                                           (addi
                                             (immediate
                                               (value->int
                                                 (var-ref %tmp.190))))
                                           (ins (cached #t) (n 1)))
                                         (var-ref %tmp.190)))
                                     (let* (((%tmp.192 (tunsigned 65535)) (safe-cast
                                                                            (tunsigned
                                                                              65535)
                                                                            (tunsigned
                                                                              1)
                                                                            '1)))
                                       (public-ledger %tally_no.191 update (4) increment
                                         (ttuple)
                                         (instructions
                                           (idx (cached #f)
                                                (pushPath #t)
                                                (path ((align 4 1))))
                                           (addi
                                             (immediate
                                               (value->int
                                                 (var-ref %tmp.192))))
                                           (ins (cached #t) (n 1)))
                                         (var-ref %tmp.192))))
                                 (public-ledger %revealed.185 update (8) insert (ttuple)
                                   (instructions
                                     (idx (cached #f)
                                          (pushPath #t)
                                          (path ((align 8 1))))
                                     (push
                                       (storage #f)
                                       (value
                                         (state-value
                                           cell
                                           (var-ref %rev_nul.186))))
                                     (push
                                       (storage #t)
                                       (value (state-value null)))
                                     (ins (cached #f) (n 1))
                                     (ins (cached #t) (n 1)))
                                   (var-ref %rev_nul.186))
                                 (call %private$state$advance.170)))))))))
        (return (tuple))))
 (circuit %advance.137 (exported #t) (pure #f) (proof #t) () (ttuple)
   (seq (let* (((%sk.161 (tbytes 32)) (call
                                        %private$secret_key.148)))
          (let* (((%apk.164 (tbytes 32)) (call
                                           %public_key.138
                                           (var-ref %sk.161))))
            (seq (assert
                   (== (tbytes 32)
                       (var-ref %apk.164)
                       (public-ledger %authority.154 read (0) read (tbytes 32)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 0 1))))
                           (popeq (cached #f) (result (void))))))
                   "Attempted to advance state without authorization")
                 (assert
                   (elt-ref
                     (public-ledger %topic.151 read (2) read
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
                 (let* (((%tmp.163
                           (tenum PublicState setup commit reveal final)) (call
                                                                            %successor.162
                                                                            (public-ledger
                                                                              %state.155
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
                   (public-ledger %state.155 write (1) write (ttuple)
                     (instructions
                       (push
                         (storage #f)
                         (value (state-value cell (align 1 1))))
                       (push
                         (storage #t)
                         (value (state-value cell (var-ref %tmp.163))))
                       (ins (cached #f) (n 1)))
                     (var-ref %tmp.163))))))
        (return (tuple))))
 (circuit %successor.162 (exported #f) (pure #t) (proof #f)
   ((%state.165 (tenum PublicState setup commit reveal final)))
   (tenum PublicState setup commit reveal final)
   (if (== (tenum PublicState setup commit reveal final)
           (var-ref %state.165)
           (enum-ref
             (tenum PublicState setup commit reveal final)
             setup))
       (return
         (enum-ref
           (tenum PublicState setup commit reveal final)
           commit))
       (if (== (tenum PublicState setup commit reveal final)
               (var-ref %state.165)
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
 (circuit %set_topic.134 (exported #t) (pure #f) (proof #t)
   ((%t.150 (topaque "string"))) (ttuple)
   (seq (let* (((%sk.149 (tbytes 32)) (call
                                        %private$secret_key.148)))
          (let* (((%apk.153 (tbytes 32)) (call
                                           %public_key.138
                                           (var-ref %sk.149))))
            (seq (assert
                   (== (tbytes 32)
                       (var-ref %apk.153)
                       (public-ledger %authority.154 read (0) read (tbytes 32)
                         (instructions
                           (dup (n 0))
                           (idx (cached #f)
                                (pushPath #f)
                                (path ((align 0 1))))
                           (popeq (cached #f) (result (void))))))
                   "Attempted to set topic without authorization")
                 (assert
                   (== (tenum PublicState setup commit reveal final)
                       (public-ledger %state.155 read (1) read
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
                 (let* (((%tmp.152
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
                                                                 %t.150))))
                   (public-ledger %topic.151 write (2) write (ttuple)
                     (instructions
                       (push
                         (storage #f)
                         (value (state-value cell (align 2 1))))
                       (push
                         (storage #t)
                         (value (state-value cell (var-ref %tmp.152))))
                       (ins (cached #f) (n 1)))
                     (var-ref %tmp.152))))))
        (return (tuple))))
 (circuit %add_voter.136 (exported #t) (pure #f) (proof #t)
   ((%pk.158 (tbytes 32))) (ttuple)
   (seq (seq (assert
               (if (elt-ref
                     (call
                       %context$eligible_voters$path_of.160
                       (var-ref %pk.158))
                     is_some
                     0)
                   '#f
                   '#t)
               "Attempted to add a voter twice")
             (let* (((%sk.156 (tbytes 32)) (call
                                             %private$secret_key.148)))
               (let* (((%apk.159 (tbytes 32)) (call
                                                %public_key.138
                                                (var-ref %sk.156))))
                 (seq (assert
                        (== (tbytes 32)
                            (var-ref %apk.159)
                            (public-ledger %authority.154 read (0) read (tbytes 32)
                              (instructions
                                (dup (n 0))
                                (idx (cached #f)
                                     (pushPath #f)
                                     (path ((align 0 1))))
                                (popeq (cached #f) (result (void))))))
                        "Attempted to add a voter without authorization")
                      (assert
                        (== (tenum PublicState setup commit reveal final)
                            (public-ledger %state.155 read (1) read
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
                      (public-ledger %eligible_voters.157 update (6) insert (ttuple)
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
                                (leaf-hash (var-ref %pk.158)))))
                          (ins (cached #f) (n 1)) (ins (cached #t) (n 1))
                          (idx (cached #f)
                               (pushPath #t)
                               (path ((align 1 1))))
                          (addi (immediate 1)) (ins (cached #t) (n 2)))
                        (var-ref %pk.158))))))
        (return (tuple))))
 (circuit %commitment_nullifier.144 (exported #f) (pure #t)
   (proof #f) ((%sk.145 (tbytes 32))) (tbytes 32)
   (return
     (call
       %persistentHash.139
       (tuple
         (single
           '#vu8(108 97 114 101 115 58 101 108 101 99 116 105 111 110
                 58 99 109 45 110 117 108 58 0 0 0 0 0 0 0 0 0 0))
         (single (var-ref %sk.145))))))
 (circuit %reveal_nullifier.146 (exported #f) (pure #t) (proof #f)
   ((%sk.147 (tbytes 32))) (tbytes 32)
   (return
     (call
       %persistentHash.139
       (tuple
         (single
           '#vu8(108 97 114 101 115 58 101 108 101 99 116 105 111 110
                 58 114 118 45 110 117 108 58 0 0 0 0 0 0 0 0 0 0))
         (single (var-ref %sk.147))))))
 (circuit %public_key.138 (exported #f) (pure #t) (proof #f)
   ((%sk.140 (tbytes 32))) (tbytes 32)
   (return
     (call
       %persistentHash.139
       (tuple
         (single
           '#vu8(108 97 114 101 115 58 101 108 101 99 116 105 111 110
                 58 112 107 58 0 0 0 0 0 0 0 0 0 0 0 0 0 0))
         (single (var-ref %sk.140))))))
 (circuit %commit_with_sk.141 (exported #f) (pure #t) (proof #f)
   ((%ballot.142 (tbytes 32)) (%sk.143 (tbytes 32)))
   (tbytes 32)
   (return
     (call
       %persistentHash.139
       (tuple
         (single (var-ref %ballot.142))
         (single (var-ref %sk.143)))))))
