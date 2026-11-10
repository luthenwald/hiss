module Monads where

import Prelude hiding (Functor(..), Applicative(..), Monad(..), Maybe(..))

data Maybe a = Nothing | Just a
   deriving (Show)

class Functor f where
   fmap :: (a -> b) -> f a -> f b

class Functor f => Applicative f where
   pure  :: a -> f a
   (<*>) :: f (a -> b) -> f a -> f b

class Applicative m => Monad m where
   (>>=)  :: m a     -> (a -> m b) -> m b
   (>>)   :: m a     -> m b        -> m b
<|>
